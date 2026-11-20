//! Conversion candidates of a Rust file (ADR-021), read from `rustc_lexer` tokens: `impl From/TryFrom<A> for B`,
//! an `impl` of another trait for a type, and functions named like conversions, with where each named type comes
//! from (a `use` declaration, a qualified path, the file itself). Glob imports, macro bodies and re-exports are not
//! followed: such a type's origin is not determined, never guessed.

use std::collections::HashMap;

use ra_ap_rustc_lexer::TokenKind;

use super::{Lexed, PRELUDE, Paths, Scope, Token, is_comment, is_ident, lex, name, scan};

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

/// Keywords that are never a type path.
const NOT_TYPES: &[&str] = &[
    "as", "async", "const", "crate", "dyn", "extern", "fn", "for", "impl", "move", "mut", "ref",
    "Self", "self", "static", "super", "unsafe", "where",
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
    /// Not determined with certainty (a glob import, an ambiguous or missing binding, a generic parameter).
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
/// bodies; empty when the file does not lex.
pub fn conversion_candidates(text: &str, added: &[u32]) -> Vec<Candidate> {
    let Some(lexed) = lex(text) else {
        return Vec::new();
    };
    let mut paths = Paths::default();
    let mut scope = Scope::default();
    scan(&lexed, &mut paths, &mut scope, None);
    let resolver = Resolver::new(&lexed, &paths, &scope);
    let code: Vec<&Token<'_>> = lexed
        .tokens
        .iter()
        .filter(|t| !is_comment(t.kind))
        .collect();
    let mut out = Vec::new();
    for (index, token) in code.iter().enumerate() {
        if token.kind != TokenKind::Ident
            || lexed.in_macro.get(token.first).copied().unwrap_or(true)
        {
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
            out.push(Candidate {
                kind,
                line,
                type_name: type_name.to_owned(),
                written: path.join("::").replace("::::", "::"),
                origin: resolver.origin(&path),
            });
        }
    }
    out
}

/// Line of the definition (`struct`, `enum`, `union`, `trait`, then `type`) of `type_name` in `text`, outside macro
/// bodies; `None` when absent or the file does not lex.
pub fn definition_line(text: &str, type_name: &str) -> Option<u32> {
    let lexed = lex(text)?;
    let code: Vec<&Token<'_>> = lexed
        .tokens
        .iter()
        .filter(|t| !is_comment(t.kind))
        .collect();
    let mut alias = None;
    for (index, token) in code.iter().enumerate() {
        if token.kind != TokenKind::Ident
            || !TYPE_ITEMS.contains(&token.text)
            || lexed.in_macro.get(token.first).copied().unwrap_or(true)
        {
            continue;
        }
        let Some(next) = code.get(index + 1).filter(|t| is_ident(t.kind)) else {
            continue;
        };
        if name(next.text) != type_name {
            continue;
        }
        let line = u32::try_from(token.first + 1).ok()?;
        if token.text == "type" {
            alias = alias.or(Some(line));
        } else {
            return Some(line);
        }
    }
    alias
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

/// The candidates of the impl header at code token `index`.
fn impl_header<'a>(code: &[&Token<'a>], index: usize) -> Vec<(CandidateKind, Vec<&'a str>)> {
    let header = header_tokens(code, index + 1);
    let mut rest: &[&Token<'a>] = &header;
    if rest.first().is_some_and(|t| t.kind == TokenKind::Lt) {
        let Some(end) = angle_end(rest, 0) else {
            return Vec::new();
        };
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
    let self_part = match self_part
        .iter()
        .position(|t| t.kind == TokenKind::Ident && t.text == "where")
    {
        Some(end) => &self_part[..end],
        None => self_part,
    };
    let self_types = type_paths(self_part);
    let generic_at = trait_part.iter().position(|t| t.kind == TokenKind::Lt);
    let trait_name = trait_part
        .get(..generic_at.unwrap_or(trait_part.len()))
        .and_then(|segments| segments.iter().rev().find(|t| is_ident(t.kind)))
        .map(|t| name(t.text));
    let kind = match trait_name {
        Some("From") => CandidateKind::FromImpl,
        Some("TryFrom") => CandidateKind::TryFromImpl,
        _ => {
            return self_types
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
        .chain(self_types)
        .map(|path| (kind, path))
        .collect()
}

/// The candidates of the function at code token `index` when its name is a conversion name: the type paths of its
/// signature, error types (`…Error`) left out.
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
    let mut signature: &[&Token<'a>] = &header_tokens(code, index + 2);
    let owned;
    let mut generics: Vec<&str> = Vec::new();
    if signature.first().is_some_and(|t| t.kind == TokenKind::Lt) {
        let Some(end) = angle_end(signature, 0) else {
            return Vec::new();
        };
        generics = signature[..end]
            .iter()
            .filter(|t| is_ident(t.kind))
            .map(|t| name(t.text))
            .collect();
        owned = signature.get(end + 1..).unwrap_or_default().to_vec();
        signature = &owned;
    }
    type_paths(signature)
        .into_iter()
        .filter(|path| {
            let last = path.last().copied().unwrap_or_default();
            !last.ends_with("Error") && !(path.len() == 1 && generics.contains(&last))
        })
        .map(|path| (CandidateKind::ConversionFn, path))
        .collect()
}

/// Code tokens from `from` up to (not including) the first `{` or `;` at the starting depth.
fn header_tokens<'t, 'a>(code: &[&'t Token<'a>], from: usize) -> Vec<&'t Token<'a>> {
    let depth = code.get(from).map_or(0, |t| t.depth);
    code.iter()
        .skip(from)
        .take_while(|t| {
            !(t.depth == depth && matches!(t.kind, TokenKind::OpenBrace | TokenKind::Semi))
        })
        .copied()
        .collect()
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

/// Resolves the first segment of a path through the file's `use` declarations and items.
struct Resolver<'s, 'a> {
    scope: &'s Scope<'a>,
    /// Path id → (parent id, segment).
    parents: HashMap<u32, (u32, &'a str)>,
    /// Bound name → line of its binding token (first one).
    binding_lines: HashMap<&'a str, u32>,
}

impl<'s, 'a> Resolver<'s, 'a> {
    fn new(lexed: &Lexed<'a>, paths: &Paths<'a>, scope: &'s Scope<'a>) -> Self {
        let parents = paths
            .ids
            .iter()
            .map(|(&(parent, segment), &id)| (id, (parent, segment)))
            .collect();
        let mut binding_lines = HashMap::new();
        for token in &lexed.tokens {
            if scope.binding_tokens.contains(&token.text.as_ptr().addr())
                && let Ok(line) = u32::try_from(token.first + 1)
            {
                binding_lines.entry(name(token.text)).or_insert(line);
            }
        }
        Self {
            scope,
            parents,
            binding_lines,
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

    fn origin(&self, path: &[&'a str]) -> TypeOrigin {
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
        match self.scope.bound.get(first) {
            Some(Some(id)) => {
                let import_line = self.binding_lines.get(first).copied();
                match self.use_path(*id).as_slice() {
                    ["crate" | "self" | "super", ..] => TypeOrigin::Local,
                    ["::", krate, ..] | [krate, _, ..] => TypeOrigin::Crate {
                        code_name: (*krate).to_owned(),
                        import_line,
                    },
                    // `use dpp;` binds a crate name: the path names that crate.
                    [krate] if path.len() > 1 => TypeOrigin::Crate {
                        code_name: (*krate).to_owned(),
                        import_line,
                    },
                    _ => TypeOrigin::NotDetermined,
                }
            }
            Some(None) => TypeOrigin::NotDetermined,
            None if self.scope.items.contains(first) => TypeOrigin::Local,
            None if path.len() > 1 => TypeOrigin::Crate {
                code_name: first.to_owned(),
                import_line: None,
            },
            None => TypeOrigin::NotDetermined,
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
}
