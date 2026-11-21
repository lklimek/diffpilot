//! Conversion candidates of a Rust file (ADR-021), read from `rustc_lexer` tokens: `impl From/TryFrom<A> for B`,
//! an `impl` of another trait for a type, and functions named like conversions, with where each named type comes
//! from (a `use` declaration in scope, a qualified path, the file itself). Glob imports, macro bodies and re-exports
//! are not followed: such a type's origin is not determined, never guessed. Work is linear in the tokens: an item
//! header longer than [`MAX_HEADER_TOKENS`] or without `{`/`;` is skipped, and candidates stop at [`MAX_CANDIDATES`].

use std::collections::{HashMap, HashSet};

use ra_ap_rustc_lexer::TokenKind;

use super::{
    PRELUDE, Paths, Scope, TestItems, Token, is_close, is_comment, is_ident, is_open, lex, name,
    scan,
};

/// Function name prefixes read as conversions.
pub const CONVERSION_PREFIXES: &[&str] = &[
    "parse_",
    "validate_",
    "from_",
    "try_from_",
    "to_",
    "into_",
    "convert_",
    "decode_",
    "encode_",
];

/// Most candidates of one file; the rest are cut.
pub const MAX_CANDIDATES: usize = 4096;
/// Most code tokens of one `impl` or `fn` header; a longer header is skipped.
pub const MAX_HEADER_TOKENS: usize = 512;

/// Keywords that are never a type path.
const NOT_TYPES: &[&str] = &[
    "as", "async", "const", "crate", "dyn", "extern", "fn", "for", "impl", "move", "mut", "ref",
    "Self", "self", "static", "super", "unsafe", "where",
];

/// Keywords a `!` may follow without a macro call (`return !x`, `if !(a)`).
const NOT_MACROS: &[&str] = &[
    "return", "if", "while", "match", "in", "break", "yield", "else", "let", "mut", "as", "move",
    "loop", "for", "await",
];

/// Item keywords whose following name is a type definition.
const TYPE_ITEMS: &[&str] = &["struct", "enum", "union", "trait", "type"];

/// What a candidate is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateKind {
    /// `impl From<A> for B`.
    FromImpl,
    /// `impl TryFrom<A> for B`.
    TryFromImpl,
    /// `impl Trait for B` with another trait.
    ImplOnType,
    /// A function named like a conversion ([`CONVERSION_PREFIXES`]).
    ConversionFn,
}

/// Where a named type comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeOrigin {
    /// A crate the file names by `code_name` (a dependency, `std`, …), through a `use` declaration at
    /// `import_line` or a qualified path.
    Crate {
        /// The crate's name in code.
        code_name: String,
        /// Line of the `use` binding, if the type came through one.
        import_line: Option<u32>,
    },
    /// The file's own crate (`crate::`, `self::`, `super::`, an item of the file).
    Local,
    /// Not determined with certainty (a glob import, an ambiguous or missing binding, a name both imported and
    /// declared in the file, a generic parameter).
    NotDetermined,
}

/// A type named by a conversion on an added line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// What names it.
    pub kind: CandidateKind,
    /// Line of the `impl` or `fn` keyword.
    pub line: u32,
    /// Last path segment: the type's name.
    pub type_name: String,
    /// The path as written (`dpp::Identity`, `Identity`).
    pub written: String,
    /// Where the type comes from.
    pub origin: TypeOrigin,
}

/// Conversion candidates whose `impl` or `fn` keyword is on one of the sorted `added` lines of `text`, outside macro
/// bodies, at most [`MAX_CANDIDATES`]; empty when the file does not lex.
pub fn conversion_candidates(text: &str, added: &[u32]) -> Vec<Candidate> {
    conversion_candidates_bounded(text, added).0
}

/// [`conversion_candidates`] and whether candidates were cut at [`MAX_CANDIDATES`].
pub fn conversion_candidates_bounded(text: &str, added: &[u32]) -> (Vec<Candidate>, bool) {
    let Some(lexed) = lex(text) else {
        return (Vec::new(), false);
    };
    let mut paths = Paths::default();
    let mut scope = Scope::default();
    scan(&lexed, &TestItems::none(), &mut paths, &mut scope, None);
    let code: Vec<&Token<'_>> = lexed
        .tokens
        .iter()
        .filter(|t| !is_comment(t.kind))
        .collect();
    let in_macro = macro_tokens(&code);
    let resolver = Resolver::new(&code, &in_macro, &paths, &scope);
    let mut out = Vec::new();
    for (index, token) in code.iter().enumerate() {
        if token.kind != TokenKind::Ident || in_macro[index] {
            continue;
        }
        let Some(line) = u32::try_from(token.first + 1)
            .ok()
            .filter(|l| added.binary_search(l).is_ok())
        else {
            continue;
        };
        let found = match token.text {
            "impl" if item_start(&code, index) => impl_header(&code, index),
            "fn" => conversion_fn(&code, index),
            _ => continue,
        };
        for (kind, path) in found {
            let Some(type_name) = path.last().copied() else {
                continue;
            };
            if out.len() >= MAX_CANDIDATES {
                return (out, true);
            }
            out.push(Candidate {
                kind,
                line,
                type_name: type_name.to_owned(),
                written: path.join("::").replace("::::", "::"),
                origin: resolver.origin(&path, token.start),
            });
        }
    }
    (out, false)
}

/// Line of the definition (`struct`, `enum`, `union`, `trait`, then `type`) of `type_name` in `text`, outside macro
/// bodies; `None` when absent or the file does not lex.
pub fn definition_line(text: &str, type_name: &str) -> Option<u32> {
    definition_lines(text, &[type_name]).remove(type_name)
}

/// [`definition_line`] of each of `names`, from one pass over the file; names without a definition are absent.
pub fn definition_lines(text: &str, names: &[&str]) -> HashMap<String, u32> {
    let Some(lexed) = lex(text) else {
        return HashMap::new();
    };
    let wanted: HashSet<&str> = names.iter().copied().collect();
    let code: Vec<&Token<'_>> = lexed
        .tokens
        .iter()
        .filter(|t| !is_comment(t.kind))
        .collect();
    let in_macro = macro_tokens(&code);
    let mut items: HashMap<String, u32> = HashMap::new();
    let mut aliases: HashMap<String, u32> = HashMap::new();
    for (index, token) in code.iter().enumerate() {
        if token.kind != TokenKind::Ident || !TYPE_ITEMS.contains(&token.text) || in_macro[index] {
            continue;
        }
        let Some(next) = code.get(index + 1).filter(|t| is_ident(t.kind)) else {
            continue;
        };
        let found = name(next.text);
        let Ok(line) = u32::try_from(token.first + 1) else {
            continue;
        };
        if !wanted.contains(found) {
            continue;
        }
        let into = if token.text == "type" {
            &mut aliases
        } else {
            &mut items
        };
        into.entry(found.to_owned()).or_insert(line);
    }
    for (alias, line) in aliases {
        items.entry(alias).or_insert(line);
    }
    items
}

/// Per code token: inside a macro token tree (the delimited group after `name!` or `macro_rules! name`, delimiters
/// included). Linear: each tree is skipped once marked.
fn macro_tokens(code: &[&Token<'_>]) -> Vec<bool> {
    let mut inside = vec![false; code.len()];
    let mut index = 0;
    while index < code.len() {
        let token = code[index];
        let bang = code
            .get(index + 1)
            .is_some_and(|t| t.kind == TokenKind::Bang);
        if !is_ident(token.kind) || !bang || NOT_MACROS.contains(&name(token.text)) {
            index += 1;
            continue;
        }
        let open_at = if name(token.text) == "macro_rules"
            && code.get(index + 2).is_some_and(|t| is_ident(t.kind))
        {
            index + 3
        } else {
            index + 2
        };
        let Some(open) = code.get(open_at).filter(|t| is_open(t.kind)) else {
            index += 1;
            continue;
        };
        let close_depth = open.depth + 1;
        let end = code
            .iter()
            .enumerate()
            .skip(open_at + 1)
            .find(|(_, t)| is_close(t.kind) && t.depth == close_depth)
            .map_or(code.len() - 1, |(at, _)| at);
        for flag in &mut inside[open_at..=end] {
            *flag = true;
        }
        index = end + 1;
    }
    inside
}

/// Whether the `impl` at code token `index` starts an item (not `impl Trait` in a type position).
fn item_start(code: &[&Token<'_>], index: usize) -> bool {
    let Some(before) = index.checked_sub(1).and_then(|i| code.get(i)) else {
        return true;
    };
    match before.kind {
        TokenKind::Semi
        | TokenKind::CloseBrace
        | TokenKind::OpenBrace
        | TokenKind::CloseBracket => true,
        TokenKind::Ident => matches!(before.text, "unsafe" | "default"),
        _ => false,
    }
}

/// The candidates of the impl header at code token `index`: the type paths of `From`/`TryFrom` arguments and the
/// outermost path of the self type (its generic arguments do not make it another crate's type); impl generic
/// parameters are never candidates.
fn impl_header<'a>(code: &[&Token<'a>], index: usize) -> Vec<(CandidateKind, Vec<&'a str>)> {
    let Some(header) = header_tokens(code, index + 1) else {
        return Vec::new();
    };
    let mut rest: &[&Token<'a>] = &header;
    let mut generics: Vec<&str> = Vec::new();
    if rest.first().is_some_and(|t| t.kind == TokenKind::Lt) {
        let Some(end) = angle_end(rest, 0) else {
            return Vec::new();
        };
        generics = generic_names(&rest[..end]);
        rest = rest.get(end + 1..).unwrap_or_default();
    }
    let mut depth = 0usize;
    let mut split = None;
    for (i, token) in rest.iter().enumerate() {
        match token.kind {
            TokenKind::Lt => depth += 1,
            TokenKind::Gt if !arrow(rest, i) => depth = depth.saturating_sub(1),
            TokenKind::Ident if depth == 0 && token.text == "for" => {
                split = Some(i);
                break;
            }
            _ => {}
        }
    }
    let Some(split) = split else {
        return Vec::new();
    };
    let (trait_part, self_part) = (&rest[..split], &rest[split + 1..]);
    let self_part = until_where(self_part);
    let not_generic =
        |path: &Vec<&str>| !(path.len() == 1 && generics.contains(path.first().unwrap_or(&"")));
    let self_type: Vec<Vec<&'a str>> = outer_path(self_part)
        .into_iter()
        .filter(not_generic)
        .collect();
    let generic_at = trait_part.iter().position(|t| t.kind == TokenKind::Lt);
    let trait_name = trait_part
        .get(..generic_at.unwrap_or(trait_part.len()))
        .and_then(|segments| segments.iter().rev().find(|t| is_ident(t.kind)))
        .map(|t| name(t.text));
    let kind = match trait_name {
        Some("From") => CandidateKind::FromImpl,
        Some("TryFrom") => CandidateKind::TryFromImpl,
        _ => {
            return self_type
                .into_iter()
                .map(|path| (CandidateKind::ImplOnType, path))
                .collect();
        }
    };
    let arguments = generic_at
        .map(|at| &trait_part[at..])
        .map(type_paths)
        .unwrap_or_default();
    arguments
        .into_iter()
        .filter(not_generic)
        .chain(self_type)
        .map(|path| (kind, path))
        .collect()
}

/// The outermost path of a type (after `&`, lifetimes, `mut` and `dyn`), if it is a path and not a prelude or
/// primitive name: `Wrapper` for `Wrapper<dpp::X>`, nothing for `Vec<dpp::X>`, `[X; 2]` or `(A, B)`.
fn outer_path<'a>(tokens: &[&Token<'a>]) -> Option<Vec<&'a str>> {
    let start = tokens.iter().position(|t| match t.kind {
        TokenKind::And | TokenKind::Lifetime { .. } => false,
        TokenKind::Ident => !matches!(t.text, "mut" | "dyn"),
        _ => true,
    })?;
    let first = tokens[start];
    if !(is_ident(first.kind) || double_colon(tokens, start)) {
        return None;
    }
    let path = type_paths(&tokens[start..]).into_iter().next()?;
    let expected = if is_ident(first.kind) {
        name(first.text)
    } else {
        "::"
    };
    (path.first() == Some(&expected)).then_some(path)
}

/// The candidates of the function at code token `index` when its name is a conversion name: the type paths of its
/// parameters and return type, without generic parameters and their bounds (in `<…>` or `where`) and without the
/// error type of a `Result<T, E>`.
fn conversion_fn<'a>(code: &[&Token<'a>], index: usize) -> Vec<(CandidateKind, Vec<&'a str>)> {
    let Some(fn_name) = code
        .get(index + 1)
        .filter(|t| is_ident(t.kind))
        .map(|t| name(t.text))
    else {
        return Vec::new();
    };
    if !CONVERSION_PREFIXES
        .iter()
        .any(|prefix| fn_name.starts_with(prefix))
    {
        return Vec::new();
    }
    let Some(header) = header_tokens(code, index + 2) else {
        return Vec::new();
    };
    let mut signature: &[&Token<'a>] = &header;
    let mut generics: Vec<&str> = Vec::new();
    if signature.first().is_some_and(|t| t.kind == TokenKind::Lt) {
        let Some(end) = angle_end(signature, 0) else {
            return Vec::new();
        };
        generics = generic_names(&signature[..end]);
        signature = signature.get(end + 1..).unwrap_or_default();
    }
    let signature = without_error_types(until_where(signature));
    type_paths(&signature)
        .into_iter()
        .filter(|path| !(path.len() == 1 && generics.contains(path.first().unwrap_or(&""))))
        .map(|path| (CandidateKind::ConversionFn, path))
        .collect()
}

/// Names declared in a generic parameter list (`<T: Bound, 'a, const N: usize>` gives `T`, `N`).
fn generic_names<'a>(list: &[&Token<'a>]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut angle = 0usize;
    let mut expect_name = true;
    for (i, token) in list.iter().enumerate() {
        match token.kind {
            TokenKind::Lt => {
                angle += 1;
                if angle > 1 {
                    expect_name = false;
                }
            }
            TokenKind::Gt if !arrow(list, i) => angle = angle.saturating_sub(1),
            TokenKind::Comma if angle == 1 => expect_name = true,
            TokenKind::Ident if angle == 1 && expect_name && token.text == "const" => {}
            TokenKind::Ident | TokenKind::RawIdent if angle == 1 && expect_name => {
                out.push(name(token.text));
                expect_name = false;
            }
            _ if angle == 1 => expect_name = false,
            _ => {}
        }
    }
    out
}

/// `tokens` up to a `where` clause at their own depth.
fn until_where<'t, 'a>(tokens: &'t [&'t Token<'a>]) -> &'t [&'t Token<'a>] {
    let depth = tokens.first().map_or(0, |t| t.depth);
    let mut angle = 0usize;
    for (i, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::Lt => angle += 1,
            TokenKind::Gt if !arrow(tokens, i) => angle = angle.saturating_sub(1),
            TokenKind::Ident if token.text == "where" && token.depth == depth && angle == 0 => {
                return &tokens[..i];
            }
            _ => {}
        }
    }
    tokens
}

/// `tokens` without the error argument of each `Result<T, E>`.
fn without_error_types<'t, 'a>(tokens: &[&'t Token<'a>]) -> Vec<&'t Token<'a>> {
    let mut skip = vec![false; tokens.len()];
    for (i, token) in tokens.iter().enumerate() {
        if !(is_ident(token.kind) && name(token.text) == "Result")
            || tokens.get(i + 1).is_none_or(|t| t.kind != TokenKind::Lt)
        {
            continue;
        }
        let Some(end) = angle_end(tokens, i + 1) else {
            continue;
        };
        let mut angle = 0usize;
        let comma = (i + 1..end).find(|&at| {
            match tokens[at].kind {
                TokenKind::Lt => angle += 1,
                TokenKind::Gt if !arrow(tokens, at) => angle = angle.saturating_sub(1),
                _ => {}
            }
            tokens[at].kind == TokenKind::Comma && angle == 1 && tokens[at].depth == tokens[i].depth
        });
        if let Some(comma) = comma {
            for flag in &mut skip[comma..end] {
                *flag = true;
            }
        }
    }
    tokens
        .iter()
        .zip(skip)
        .filter(|(_, skip)| !skip)
        .map(|(t, _)| *t)
        .collect()
}

/// Code tokens from `from` up to (not including) the first `{` or `;` at the starting depth; `None` when the
/// enclosing group closes or the file ends first, or after [`MAX_HEADER_TOKENS`] tokens (no item header).
fn header_tokens<'t, 'a>(code: &[&'t Token<'a>], from: usize) -> Option<Vec<&'t Token<'a>>> {
    let depth = code.get(from)?.depth;
    let mut out = Vec::new();
    for token in code.iter().skip(from) {
        if token.depth == depth && matches!(token.kind, TokenKind::OpenBrace | TokenKind::Semi) {
            return Some(out);
        }
        if (token.depth <= depth && is_close(token.kind)) || out.len() >= MAX_HEADER_TOKENS {
            return None;
        }
        out.push(*token);
    }
    None
}

/// Index of the `>` closing the `<` at `start`.
fn angle_end(tokens: &[&Token<'_>], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, token) in tokens.iter().enumerate().skip(start) {
        match token.kind {
            TokenKind::Lt => depth += 1,
            TokenKind::Gt if !arrow(tokens, i) => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether the `>` at `i` is the end of `->` or `=>`.
fn arrow(tokens: &[&Token<'_>], i: usize) -> bool {
    i.checked_sub(1)
        .and_then(|before| tokens.get(before))
        .is_some_and(|t| {
            matches!(t.kind, TokenKind::Minus | TokenKind::Eq) && t.start + 1 == tokens[i].start
        })
}

/// The type paths among `tokens`: `ident(::ident)*` (a leading `::` kept as a segment), without keywords, binding
/// names (`name:`), lifetimes and prelude or primitive names.
fn type_paths<'a>(tokens: &[&Token<'a>]) -> Vec<Vec<&'a str>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let mut segments: Vec<&'a str> = Vec::new();
        if double_colon(tokens, i) {
            segments.push("::");
            i += 2;
        }
        while let Some(token) = tokens.get(i).filter(|t| is_ident(t.kind)) {
            segments.push(name(token.text));
            i += 1;
            if double_colon(tokens, i) && tokens.get(i + 2).is_some_and(|t| is_ident(t.kind)) {
                i += 2;
            } else {
                break;
            }
        }
        if segments.is_empty() {
            i += 1;
            continue;
        }
        let binding =
            tokens.get(i).is_some_and(|t| t.kind == TokenKind::Colon) && !double_colon(tokens, i);
        let last = segments.last().copied().unwrap_or_default();
        let keyword = segments.len() == 1 && NOT_TYPES.contains(&last);
        let prelude = segments.len() == 1 && PRELUDE.contains(&last);
        let module_only = matches!(segments.as_slice(), ["::"] | ["crate" | "self" | "super"]);
        if !binding && !keyword && !prelude && !module_only && segments.last() != Some(&"::") {
            out.push(segments);
        }
    }
    out
}

fn double_colon(tokens: &[&Token<'_>], i: usize) -> bool {
    matches!(
        (tokens.get(i), tokens.get(i + 1)),
        (Some(a), Some(b)) if a.kind == TokenKind::Colon && b.kind == TokenKind::Colon && a.start + 1 == b.start
    )
}

/// A `use` binding's reach: the byte span of the block or module holding the `use` (`None` = the whole file).
type Reach = Option<(usize, usize)>;

/// Resolves the first segment of a path through the `use` declarations in scope and the file's items.
struct Resolver<'s, 'a> {
    scope: &'s Scope<'a>,
    /// Path id → (parent id, segment).
    parents: HashMap<u32, (u32, &'a str)>,
    /// Bound name → (line of its binding token, its reach), in file order.
    bindings: HashMap<&'a str, Vec<(u32, Reach)>>,
}

impl<'s, 'a> Resolver<'s, 'a> {
    fn new(
        code: &[&Token<'a>],
        in_macro: &[bool],
        paths: &Paths<'a>,
        scope: &'s Scope<'a>,
    ) -> Self {
        let parents = paths
            .ids
            .iter()
            .map(|(&(parent, segment), &id)| (id, (parent, segment)))
            .collect();
        // Byte span of each `{ … }` group, by the index of its `{`.
        let mut open: Vec<usize> = Vec::new();
        let mut spans: HashMap<usize, (usize, usize)> = HashMap::new();
        for (index, token) in code.iter().enumerate() {
            match token.kind {
                TokenKind::OpenBrace => open.push(index),
                TokenKind::CloseBrace => {
                    if let Some(start) = open.pop() {
                        spans.insert(start, (code[start].start, token.start));
                    }
                }
                _ => {}
            }
        }
        let mut bindings: HashMap<&'a str, Vec<(u32, Reach)>> = HashMap::new();
        let mut enclosing: Vec<usize> = Vec::new();
        let mut index = 0;
        while index < code.len() {
            let token = code[index];
            match token.kind {
                TokenKind::OpenBrace => enclosing.push(index),
                TokenKind::CloseBrace => {
                    enclosing.pop();
                }
                TokenKind::Ident if token.text == "use" && !in_macro[index] => {
                    let reach: Reach = enclosing.last().and_then(|at| spans.get(at).copied());
                    let mut at = index + 1;
                    while let Some(t) = code.get(at) {
                        if t.kind == TokenKind::Semi && t.depth == token.depth {
                            break;
                        }
                        if scope.binding_tokens.contains(&t.text.as_ptr().addr())
                            && let Ok(line) = u32::try_from(t.first + 1)
                        {
                            bindings
                                .entry(name(t.text))
                                .or_default()
                                .push((line, reach));
                        }
                        at += 1;
                    }
                    index = at;
                    continue;
                }
                _ => {}
            }
            index += 1;
        }
        Self {
            scope,
            parents,
            bindings,
        }
    }

    /// The segments of the `use` path with id `id`, root first.
    fn use_path(&self, mut id: u32) -> Vec<&'a str> {
        let mut segments = Vec::new();
        while let Some(&(parent, segment)) = self.parents.get(&id) {
            segments.push(segment);
            id = parent;
        }
        segments.reverse();
        segments
    }

    /// Where `path`, written at byte `at`, comes from.
    fn origin(&self, path: &[&'a str], at: usize) -> TypeOrigin {
        let Some(&first) = path.first() else {
            return TypeOrigin::NotDetermined;
        };
        match first {
            "crate" | "self" | "super" => return TypeOrigin::Local,
            "::" => {
                return path
                    .get(1)
                    .map_or(TypeOrigin::NotDetermined, |krate| TypeOrigin::Crate {
                        code_name: (*krate).to_owned(),
                        import_line: None,
                    });
            }
            _ => {}
        }
        let in_reach = self.bindings.get(first).and_then(|bindings| {
            bindings
                .iter()
                .find(|(_, reach)| reach.is_none_or(|(start, end)| start < at && at < end))
        });
        let declared = self.scope.items.contains(first);
        match (in_reach, self.scope.bound.get(first)) {
            // Imported and declared in the file under the same name: which one applies is not certain.
            (Some(_), _) if declared => TypeOrigin::NotDetermined,
            (Some(&(line, _)), Some(Some(id))) => match self.use_path(*id).as_slice() {
                ["crate" | "self" | "super", ..] => TypeOrigin::Local,
                ["::", krate, ..] | [krate, _, ..] => TypeOrigin::Crate {
                    code_name: (*krate).to_owned(),
                    import_line: Some(line),
                },
                // `use dpp;` binds a crate name: the path names that crate.
                [krate] if path.len() > 1 => TypeOrigin::Crate {
                    code_name: (*krate).to_owned(),
                    import_line: Some(line),
                },
                _ => TypeOrigin::NotDetermined,
            },
            (Some(_), _) => TypeOrigin::NotDetermined,
            // Bound only in a block or module out of reach, or not bound.
            (None, _) if declared => TypeOrigin::Local,
            (None, _) if self.scope.unsure && path.len() == 1 => TypeOrigin::NotDetermined,
            (None, _) if path.len() > 1 => TypeOrigin::Crate {
                code_name: first.to_owned(),
                import_line: None,
            },
            (None, _) => TypeOrigin::NotDetermined,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_lines(text: &str) -> Vec<u32> {
        (1..=u32::try_from(text.lines().count()).unwrap_or(0)).collect()
    }

    fn candidates(text: &str) -> Vec<Candidate> {
        conversion_candidates(text, &all_lines(text))
    }

    fn krate(code_name: &str, import_line: Option<u32>) -> TypeOrigin {
        TypeOrigin::Crate {
            code_name: code_name.to_owned(),
            import_line,
        }
    }

    fn found(text: &str) -> Vec<(CandidateKind, u32, String, TypeOrigin)> {
        candidates(text)
            .into_iter()
            .map(|c| (c.kind, c.line, c.type_name, c.origin))
            .collect()
    }

    #[test]
    fn conversions_from_impl_of_an_imported_type() {
        let text = "use dpp::state_transition::StateTransitionType;\nenum Kind {}\nimpl From<StateTransitionType> for Kind {\n    fn from(t: StateTransitionType) -> Self { Kind::A }\n}\n";
        assert_eq!(
            found(text),
            [
                (
                    CandidateKind::FromImpl,
                    3,
                    "StateTransitionType".to_owned(),
                    krate("dpp", Some(1))
                ),
                (
                    CandidateKind::FromImpl,
                    3,
                    "Kind".to_owned(),
                    TypeOrigin::Local
                ),
            ]
        );
    }

    #[test]
    fn conversions_try_from_with_a_qualified_reference() {
        let text = "struct Local;\nimpl<'a> TryFrom<&'a dpp::identity::Identity> for Local {\n    type Error = ();\n}\n";
        let got = candidates(text);
        assert_eq!(got[0].kind, CandidateKind::TryFromImpl);
        assert_eq!(got[0].type_name, "Identity");
        assert_eq!(got[0].written, "dpp::identity::Identity");
        assert_eq!(got[0].origin, krate("dpp", None));
    }

    #[test]
    fn conversions_from_a_local_type_into_a_foreign_one() {
        let text = "struct L;\nimpl From<L> for ::dashcore::Txid {}\n";
        let got = found(text);
        assert!(
            got.contains(&(
                CandidateKind::FromImpl,
                2,
                "Txid".to_owned(),
                krate("dashcore", None)
            )),
            "{got:?}"
        );
    }

    #[test]
    fn conversions_extension_trait_on_a_foreign_type() {
        let text = "use dpp::X;\ntrait XExt {}\nimpl XExt for X {}\nstruct L;\nimpl std::fmt::Display for L {}\n";
        assert_eq!(
            found(text),
            [
                (
                    CandidateKind::ImplOnType,
                    3,
                    "X".to_owned(),
                    krate("dpp", Some(1))
                ),
                (
                    CandidateKind::ImplOnType,
                    5,
                    "L".to_owned(),
                    TypeOrigin::Local
                ),
            ],
            "the trait of another impl is not a candidate"
        );
    }

    #[test]
    fn conversions_parse_function_names_its_types_but_not_errors() {
        let text = "use dpp::TransitionType;\nuse crate::errors::ParseError;\npub fn parse_transition(input: &str) -> Result<TransitionType, ParseError> {\n    todo()\n}\n";
        assert_eq!(
            found(text),
            [(
                CandidateKind::ConversionFn,
                3,
                "TransitionType".to_owned(),
                krate("dpp", Some(1))
            )]
        );
    }

    #[test]
    fn conversions_other_functions_are_not_candidates() {
        assert!(candidates("fn run(x: dpp::X) -> dpp::Y { x }\n").is_empty());
    }

    #[test]
    fn conversions_only_added_lines_count() {
        let text = "struct L;\nimpl From<dpp::X> for L {}\n";
        assert!(conversion_candidates(text, &[1]).is_empty());
        assert_eq!(conversion_candidates(text, &[2]).len(), 2);
    }

    #[test]
    fn conversions_inside_macros_are_not_read() {
        let text = "macro_rules! m {\n    () => { impl From<dpp::X> for Y {} };\n}\n";
        assert!(candidates(text).is_empty());
    }

    #[test]
    fn conversions_grouped_and_renamed_imports() {
        let text = "use dpp::{prelude::Identifier as Id, block::{Height}};\nstruct L;\nimpl From<Id> for L {}\nimpl TryFrom<Height> for L {}\n";
        let got = found(text);
        assert!(
            got.contains(&(
                CandidateKind::FromImpl,
                3,
                "Id".to_owned(),
                krate("dpp", Some(1))
            )),
            "{got:?}"
        );
        assert!(
            got.contains(&(
                CandidateKind::TryFromImpl,
                4,
                "Height".to_owned(),
                krate("dpp", Some(1))
            )),
            "{got:?}"
        );
    }

    #[test]
    fn conversions_glob_imports_leave_the_origin_open() {
        let text = "use dpp::prelude::*;\nstruct L;\nimpl From<Identifier> for L {}\n";
        let got = found(text);
        assert!(got.contains(&(
            CandidateKind::FromImpl,
            3,
            "Identifier".to_owned(),
            TypeOrigin::NotDetermined
        )));
    }

    #[test]
    fn conversions_generic_parameters_are_not_crates() {
        let text = "struct L;\nimpl<T> From<T> for L where T: Into<u8> {}\nfn to_l<U: Into<L>>(u: U) -> L { L }\n";
        assert!(
            candidates(text)
                .iter()
                .all(|c| !matches!(c.origin, TypeOrigin::Crate { .. })),
            "{:?}",
            candidates(text)
        );
    }

    #[test]
    fn conversions_impl_in_type_position_is_no_item() {
        let text = "fn to_x() -> impl Into<dpp::X> { todo() }\n";
        assert_eq!(
            found(text),
            [(
                CandidateKind::ConversionFn,
                1,
                "X".to_owned(),
                krate("dpp", None)
            )]
        );
    }

    #[test]
    fn conversions_crate_relative_paths_are_local() {
        let text = "use super::model::Thing;\nimpl From<crate::a::B> for Thing {}\n";
        assert!(
            candidates(text)
                .iter()
                .all(|c| c.origin == TypeOrigin::Local)
        );
    }

    #[test]
    fn conversions_module_import_names_the_crate() {
        let text = "use dpp::identity;\nstruct L;\nimpl From<identity::Identity> for L {}\n";
        let got = found(text);
        assert!(
            got.contains(&(
                CandidateKind::FromImpl,
                3,
                "Identity".to_owned(),
                krate("dpp", Some(1))
            )),
            "{got:?}"
        );
    }

    #[test]
    fn conversions_unlexable_file_gives_nothing() {
        assert!(candidates("impl From<dpp::X> for L { \"unterminated }\n").is_empty());
    }

    #[test]
    fn conversions_definition_lines() {
        let text = "pub type Foo = u8;\nmod m {\n    pub(crate) struct Foo;\n}\nmacro_rules! d { () => { struct Bar; } }\n";
        assert_eq!(
            definition_line(text, "Foo"),
            Some(3),
            "a struct wins over a type alias"
        );
        assert_eq!(definition_line(text, "Bar"), None, "inside a macro");
        assert_eq!(definition_line(text, "Baz"), None);
        assert_eq!(definition_line("type Only = u8;\n", "Only"), Some(1));
    }

    #[test]
    fn conversions_generic_arguments_of_the_self_type_are_not_the_impl_type() {
        let text = "struct Wrapper<T>(T);\nimpl std::fmt::Display for Wrapper<dpp::X> {}\nimpl Ext for Vec<dpp::X> {}\n";
        let got = found(text);
        assert!(got.iter().all(|(_, _, name, _)| name != "X"), "{got:?}");
    }

    #[test]
    fn conversions_bounds_in_where_and_generics_are_not_signature_types() {
        let text = "fn to_x<T>(t: T) -> X where T: dpp::Tr { todo() }\nfn to_y<T: dpp::Tr>(t: T) -> Y { todo() }\n";
        let names: Vec<String> = candidates(text).into_iter().map(|c| c.type_name).collect();
        assert_eq!(names, ["X", "Y"]);
    }

    #[test]
    fn conversions_error_types_count_except_the_error_of_a_result() {
        let text = "fn into_consensus_error(e: E) -> dpp::ConsensusError { todo() }\nfn parse_x(s: &str) -> Result<dpp::X, dpp::ParseError> { todo() }\n";
        let names: Vec<String> = candidates(text).into_iter().map(|c| c.written).collect();
        assert_eq!(names, ["E", "dpp::ConsensusError", "dpp::X"]);
    }

    #[test]
    fn conversions_impl_generic_parameters_shadow_imports() {
        let text = "use dpp::Identity;\nstruct L;\nimpl<Identity> From<Identity> for L {}\n";
        assert!(
            candidates(text).iter().all(|c| c.type_name != "Identity"),
            "{:?}",
            candidates(text)
        );
    }

    #[test]
    fn conversions_raw_identifiers_are_names() {
        let text = "use dpp::r#type::Kind;\nstruct L;\nimpl From<r#Kind> for L {}\n";
        let got = found(text);
        assert!(
            got.contains(&(
                CandidateKind::FromImpl,
                3,
                "Kind".to_owned(),
                krate("dpp", Some(1))
            )),
            "{got:?}"
        );
    }

    #[test]
    fn conversions_not_operator_after_a_keyword_is_no_macro() {
        let text =
            "fn f() -> bool {\n    return !(true);\n}\nstruct L;\nimpl From<dpp::X> for L {}\n";
        assert!(
            candidates(text).iter().any(|c| c.type_name == "X"),
            "{:?}",
            candidates(text)
        );
    }

    #[test]
    fn conversions_imports_reach_their_module_only() {
        let text = "mod a {\n    use dpp::Foo;\n    struct L;\n    impl From<Foo> for L {}\n}\nstruct M;\nimpl From<Foo> for M {}\n";
        let got = found(text);
        assert!(
            got.contains(&(
                CandidateKind::FromImpl,
                4,
                "Foo".to_owned(),
                krate("dpp", Some(2))
            )),
            "inside the module: {got:?}"
        );
        assert!(
            got.contains(&(
                CandidateKind::FromImpl,
                7,
                "Foo".to_owned(),
                TypeOrigin::NotDetermined
            )),
            "outside it: {got:?}"
        );
    }

    #[test]
    fn conversions_a_name_imported_and_declared_is_not_determined() {
        let text = "use dpp::Foo;\nmod m { pub struct Foo; }\nstruct L;\nimpl From<Foo> for L {}\n";
        let got = found(text);
        assert!(
            got.contains(&(
                CandidateKind::FromImpl,
                4,
                "Foo".to_owned(),
                TypeOrigin::NotDetermined
            )),
            "{got:?}"
        );
    }

    #[test]
    fn conversions_definition_lines_in_one_pass() {
        let text = "type A = u8;\nstruct A;\nenum B {}\nmacro_rules! m { () => { struct C; } }\n";
        let lines = definition_lines(text, &["A", "B", "C", "D"]);
        assert_eq!(lines.get("A"), Some(&2));
        assert_eq!(lines.get("B"), Some(&3));
        assert_eq!(lines.len(), 2);
    }
}
