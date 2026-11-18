//! A small GraphQL document parser for the dashboard documents: operations with variable definitions,
//! fragments, inline fragments and fields with aliases and arguments; directives are skipped. `Document::resolve`
//! then validates like GitHub before execution: variables (declared, required, typed, defaults), fragments
//! (defined, no cycles) and the expanded size.

use std::collections::HashMap;
use std::fmt;

use serde_json::{Map, Number, Value};

/// Deepest nesting of selection sets and values accepted.
const MAX_DEPTH: usize = 64;
/// Most field selections a document may expand to (fragments counted at every use).
const MAX_FIELDS: usize = 20_000;
/// Key of the placeholder a `$variable` takes until `Document::resolve`; GraphQL object keys cannot contain `$`.
const VARIABLE: &str = "$variable";

/// The document is not syntactically valid GraphQL.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ParseError {
    /// The document ends inside a definition.
    #[error("unexpected end of the GraphQL document")]
    End,
    /// A token that does not fit the grammar.
    #[error("unexpected `{found}` at byte {at} of the GraphQL document")]
    Unexpected {
        /// The token text.
        found: String,
        /// Byte offset of the token.
        at: usize,
    },
    /// A string literal without its closing quote, or with a bad escape.
    #[error("unterminated string or invalid escape at byte {at} of the GraphQL document")]
    Unterminated {
        /// Byte offset of the opening quote.
        at: usize,
    },
    /// Only fragments, no operation.
    #[error("the GraphQL document has no operation")]
    NoOperation,
    /// Selection sets or values nested deeper than `MAX_DEPTH`.
    #[error("the GraphQL document nests deeper than {MAX_DEPTH} levels at byte {at}")]
    TooDeep {
        /// Byte offset where the limit was passed.
        at: usize,
    },
}

/// The document is GraphQL but GitHub would refuse it before executing anything.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum DocumentError {
    /// Syntax error.
    #[error("Parse error: {0}")]
    Parse(#[from] ParseError),
    /// A required variable is absent, or a variable has a value of the wrong type.
    #[error("Variable ${name} of type {ty} was provided invalid value")]
    Variable {
        /// Variable name.
        name: String,
        /// Declared type.
        ty: String,
    },
    /// A `$variable` used but not declared by the operation.
    #[error("Variable ${name} is used by the operation but not declared")]
    Undeclared {
        /// Variable name.
        name: String,
    },
    /// A spread of an undefined fragment.
    #[error("Fragment {name} was used, but not defined")]
    UnknownFragment {
        /// Fragment name.
        name: String,
    },
    /// A fragment that spreads itself, directly or not.
    #[error("Fragment {name} contains an infinite loop")]
    FragmentLoop {
        /// Fragment name.
        name: String,
    },
    /// More than `MAX_FIELDS` field selections once fragments are expanded.
    #[error("The query expands to more than {MAX_FIELDS} fields")]
    TooLarge,
}

/// Operation type of the document's first operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Query,
    Mutation,
    Subscription,
}

/// A variable's declared type.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Type {
    Named { name: String, required: bool },
    List { item: Box<Type>, required: bool },
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (text, required) = match self {
            Self::Named { name, required } => (name.clone(), *required),
            Self::List { item, required } => (format!("[{item}]"), *required),
        };
        write!(f, "{text}{}", if required { "!" } else { "" })
    }
}

impl Type {
    fn required(&self) -> bool {
        match self {
            Self::Named { required, .. } | Self::List { required, .. } => *required,
        }
    }

    /// Whether `value` coerces to the type (enums and input objects accept any non-null value).
    fn accepts(&self, value: &Value) -> bool {
        if value.is_null() {
            return !self.required();
        }
        match self {
            Self::List { item, .. } => match value {
                Value::Array(items) => items.iter().all(|v| item.accepts(v)),
                single => item.accepts(single),
            },
            Self::Named { name, .. } => match name.as_str() {
                "String" | "URI" | "DateTime" | "GitTimestamp" | "GitObjectID" | "HTML" => {
                    value.is_string()
                }
                "ID" => value.is_string() || value.is_i64(),
                "Int" => value.is_i64(),
                "Float" => value.is_number(),
                "Boolean" => value.is_boolean(),
                _ => true,
            },
        }
    }
}

/// A declared variable.
#[derive(Debug)]
struct Definition {
    name: String,
    ty: Type,
    default: Option<Value>,
}

/// A parsed document: its first operation and every fragment definition.
#[derive(Debug)]
pub(crate) struct Document {
    pub(crate) kind: Kind,
    pub(crate) selections: Vec<Selection>,
    pub(crate) fragments: HashMap<String, Fragment>,
    definitions: Vec<Definition>,
}

/// A named fragment.
#[derive(Debug)]
pub(crate) struct Fragment {
    pub(crate) on: String,
    pub(crate) selections: Vec<Selection>,
}

/// One entry of a selection set.
#[derive(Debug)]
pub(crate) enum Selection {
    Field(Field),
    Spread(String),
    Inline {
        on: Option<String>,
        selections: Vec<Selection>,
    },
}

/// A field with its arguments (variables substituted by `Document::resolve`).
#[derive(Debug)]
pub(crate) struct Field {
    pub(crate) alias: Option<String>,
    pub(crate) name: String,
    pub(crate) args: Map<String, Value>,
    pub(crate) selections: Vec<Selection>,
}

impl Field {
    /// The response key: the alias, else the field name.
    pub(crate) fn key(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Punct(char),
    Spread,
    Name(String),
    Str(String),
    Num(String),
}

impl Token {
    fn text(&self) -> String {
        match self {
            Self::Punct(c) => c.to_string(),
            Self::Spread => "...".to_owned(),
            Self::Name(s) | Self::Num(s) => s.clone(),
            Self::Str(s) => format!("\"{s}\""),
        }
    }
}

fn tokenize(src: &str) -> Result<Vec<(usize, Token)>, ParseError> {
    let bytes = src.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b' ' | b'\t' | b'\n' | b'\r' | b',' => i += 1,
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'.' if src[i..].starts_with("...") => {
                tokens.push((i, Token::Spread));
                i += 3;
            }
            b'{' | b'}' | b'(' | b')' | b':' | b'$' | b'!' | b'[' | b']' | b'=' | b'@' | b'|'
            | b'&' => {
                tokens.push((i, Token::Punct(char::from(c))));
                i += 1;
            }
            b'"' if src[i..].starts_with("\"\"\"") => {
                let start = i;
                let end = src[i + 3..]
                    .find("\"\"\"")
                    .ok_or(ParseError::Unterminated { at: start })?;
                tokens.push((start, Token::Str(src[i + 3..i + 3 + end].to_owned())));
                i += end + 6;
            }
            b'"' => {
                let start = i;
                let (text, next) =
                    string(src, i + 1).ok_or(ParseError::Unterminated { at: start })?;
                tokens.push((start, Token::Str(text)));
                i = next;
            }
            b'-' | b'0'..=b'9' => {
                let start = i;
                i += 1;
                while i < bytes.len()
                    && (bytes[i].is_ascii_digit()
                        || matches!(bytes[i], b'.' | b'e' | b'E' | b'+' | b'-'))
                {
                    i += 1;
                }
                tokens.push((start, Token::Num(src[start..i].to_owned())));
            }
            b'_' | b'A'..=b'Z' | b'a'..=b'z' => {
                let start = i;
                while i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                tokens.push((start, Token::Name(src[start..i].to_owned())));
            }
            _ => {
                let found = src[i..]
                    .chars()
                    .next()
                    .map(String::from)
                    .unwrap_or_default();
                return Err(ParseError::Unexpected { found, at: i });
            }
        }
    }
    Ok(tokens)
}

/// Four hex digits of a `\u` escape.
fn hex4(chars: &mut std::str::CharIndices<'_>) -> Option<u32> {
    let hex: String = (0..4)
        .filter_map(|_| chars.next().map(|(_, h)| h))
        .collect();
    if hex.len() == 4 {
        u32::from_str_radix(&hex, 16).ok()
    } else {
        None
    }
}

/// A quoted string starting after its opening quote: (text, index after the closing quote).
fn string(src: &str, start: usize) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut chars = src[start..].char_indices();
    while let Some((offset, c)) = chars.next() {
        match c {
            '"' => return Some((out, start + offset + 1)),
            '\n' => return None,
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'u' => {
                    let mut code = hex4(&mut chars)?;
                    if (0xD800..0xDC00).contains(&code) {
                        if chars.next()?.1 != '\\' || chars.next()?.1 != 'u' {
                            return None;
                        }
                        let low = hex4(&mut chars).filter(|low| (0xDC00..0xE000).contains(low))?;
                        code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                    }
                    out.push(char::from_u32(code)?);
                }
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

struct Parser {
    tokens: Vec<(usize, Token)>,
    pos: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|(_, token)| token)
    }

    fn next(&mut self) -> Result<Token, ParseError> {
        let token = self
            .tokens
            .get(self.pos)
            .map(|(_, t)| t.clone())
            .ok_or(ParseError::End)?;
        self.pos += 1;
        Ok(token)
    }

    fn unexpected(&self) -> ParseError {
        match self.tokens.get(self.pos) {
            Some((at, token)) => ParseError::Unexpected {
                found: token.text(),
                at: *at,
            },
            None => ParseError::End,
        }
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&Token::Punct(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: char) -> Result<(), ParseError> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(self.unexpected())
        }
    }

    fn name(&mut self) -> Result<String, ParseError> {
        match self.peek() {
            Some(Token::Name(name)) => {
                let name = name.clone();
                self.pos += 1;
                Ok(name)
            }
            _ => Err(self.unexpected()),
        }
    }

    /// Enters one nesting level; fails beyond `MAX_DEPTH`.
    fn descend(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            let at = self.tokens.get(self.pos).map_or(0, |(at, _)| *at);
            return Err(ParseError::TooDeep { at });
        }
        Ok(())
    }

    fn ty(&mut self) -> Result<Type, ParseError> {
        let ty = if self.eat('[') {
            self.descend()?;
            let item = Box::new(self.ty()?);
            self.expect(']')?;
            self.depth -= 1;
            Type::List {
                item,
                required: false,
            }
        } else {
            Type::Named {
                name: self.name()?,
                required: false,
            }
        };
        Ok(if self.eat('!') {
            match ty {
                Type::List { item, .. } => Type::List {
                    item,
                    required: true,
                },
                Type::Named { name, .. } => Type::Named {
                    name,
                    required: true,
                },
            }
        } else {
            ty
        })
    }

    fn definitions(&mut self) -> Result<Vec<Definition>, ParseError> {
        let mut definitions = Vec::new();
        if !self.eat('(') {
            return Ok(definitions);
        }
        while !self.eat(')') {
            self.expect('$')?;
            let name = self.name()?;
            self.expect(':')?;
            let ty = self.ty()?;
            let default = if self.eat('=') {
                Some(self.value()?)
            } else {
                None
            };
            self.directives()?;
            definitions.push(Definition { name, ty, default });
        }
        Ok(definitions)
    }

    fn directives(&mut self) -> Result<(), ParseError> {
        while self.eat('@') {
            self.name()?;
            if self.peek() == Some(&Token::Punct('(')) {
                self.arguments()?;
            }
        }
        Ok(())
    }

    fn document(&mut self) -> Result<Document, ParseError> {
        let mut operation = None;
        let mut fragments = HashMap::new();
        while let Some(token) = self.peek().cloned() {
            match token {
                Token::Punct('{') => {
                    let selections = self.selection_set()?;
                    operation.get_or_insert((Kind::Query, selections, Vec::new()));
                }
                Token::Name(word) if word == "fragment" => {
                    self.pos += 1;
                    let name = self.name()?;
                    if self.name()? != "on" {
                        return Err(self.unexpected());
                    }
                    let on = self.name()?;
                    self.directives()?;
                    let selections = self.selection_set()?;
                    fragments.insert(name, Fragment { on, selections });
                }
                Token::Name(word)
                    if matches!(word.as_str(), "query" | "mutation" | "subscription") =>
                {
                    self.pos += 1;
                    let kind = match word.as_str() {
                        "query" => Kind::Query,
                        "mutation" => Kind::Mutation,
                        _ => Kind::Subscription,
                    };
                    if matches!(self.peek(), Some(Token::Name(_))) {
                        self.pos += 1;
                    }
                    let definitions = self.definitions()?;
                    self.directives()?;
                    let selections = self.selection_set()?;
                    operation.get_or_insert((kind, selections, definitions));
                }
                _ => return Err(self.unexpected()),
            }
        }
        let (kind, selections, definitions) = operation.ok_or(ParseError::NoOperation)?;
        Ok(Document {
            kind,
            selections,
            fragments,
            definitions,
        })
    }

    fn selection_set(&mut self) -> Result<Vec<Selection>, ParseError> {
        self.expect('{')?;
        self.descend()?;
        let mut selections = Vec::new();
        while !self.eat('}') {
            if self.peek() == Some(&Token::Spread) {
                self.pos += 1;
                match self.peek() {
                    Some(Token::Name(word)) if word == "on" => {
                        self.pos += 1;
                        let on = Some(self.name()?);
                        self.directives()?;
                        let selections_inner = self.selection_set()?;
                        selections.push(Selection::Inline {
                            on,
                            selections: selections_inner,
                        });
                    }
                    Some(Token::Name(_)) => {
                        let name = self.name()?;
                        self.directives()?;
                        selections.push(Selection::Spread(name));
                    }
                    _ => {
                        self.directives()?;
                        let selections_inner = self.selection_set()?;
                        selections.push(Selection::Inline {
                            on: None,
                            selections: selections_inner,
                        });
                    }
                }
                continue;
            }
            let first = self.name()?;
            let (alias, name) = if self.eat(':') {
                (Some(first), self.name()?)
            } else {
                (None, first)
            };
            let args = if self.peek() == Some(&Token::Punct('(')) {
                self.arguments()?
            } else {
                Map::new()
            };
            self.directives()?;
            let selections_inner = if self.peek() == Some(&Token::Punct('{')) {
                self.selection_set()?
            } else {
                Vec::new()
            };
            selections.push(Selection::Field(Field {
                alias,
                name,
                args,
                selections: selections_inner,
            }));
        }
        self.depth -= 1;
        Ok(selections)
    }

    fn arguments(&mut self) -> Result<Map<String, Value>, ParseError> {
        self.expect('(')?;
        let mut args = Map::new();
        while !self.eat(')') {
            let name = self.name()?;
            self.expect(':')?;
            let value = self.value()?;
            args.insert(name, value);
        }
        Ok(args)
    }

    fn value(&mut self) -> Result<Value, ParseError> {
        let value = match self.next()? {
            Token::Punct('$') => {
                let mut placeholder = Map::new();
                placeholder.insert(VARIABLE.to_owned(), Value::String(self.name()?));
                Value::Object(placeholder)
            }
            Token::Str(text) => Value::String(text),
            Token::Num(text) => text
                .parse::<i64>()
                .map(Value::from)
                .ok()
                .or_else(|| {
                    text.parse::<f64>()
                        .ok()
                        .and_then(Number::from_f64)
                        .map(Value::Number)
                })
                .unwrap_or(Value::String(text)),
            Token::Name(word) => match word.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                "null" => Value::Null,
                _ => Value::String(word),
            },
            Token::Punct('[') => {
                self.descend()?;
                let mut items = Vec::new();
                while !self.eat(']') {
                    items.push(self.value()?);
                }
                self.depth -= 1;
                Value::Array(items)
            }
            Token::Punct('{') => {
                self.descend()?;
                let mut object = Map::new();
                while !self.eat('}') {
                    let key = self.name()?;
                    self.expect(':')?;
                    object.insert(key, self.value()?);
                }
                self.depth -= 1;
                Value::Object(object)
            }
            _ => {
                self.pos -= 1;
                return Err(self.unexpected());
            }
        };
        Ok(value)
    }
}

/// Parses `src` (syntax only; variables stay placeholders until `Document::resolve`).
pub(crate) fn parse(src: &str) -> Result<Document, ParseError> {
    Parser {
        tokens: tokenize(src)?,
        pos: 0,
        depth: 0,
    }
    .document()
}

/// Replaces variable placeholders in `value` by their values.
fn substitute(value: &mut Value, values: &HashMap<String, Value>) -> Result<(), DocumentError> {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(name)) = object.get(VARIABLE) {
                *value = values
                    .get(name)
                    .cloned()
                    .ok_or_else(|| DocumentError::Undeclared { name: name.clone() })?;
                return Ok(());
            }
            object
                .values_mut()
                .try_for_each(|item| substitute(item, values))
        }
        Value::Array(items) => items
            .iter_mut()
            .try_for_each(|item| substitute(item, values)),
        _ => Ok(()),
    }
}

fn substitute_all(
    selections: &mut [Selection],
    values: &HashMap<String, Value>,
) -> Result<(), DocumentError> {
    for selection in selections {
        match selection {
            Selection::Field(field) => {
                field
                    .args
                    .values_mut()
                    .try_for_each(|value| substitute(value, values))?;
                substitute_all(&mut field.selections, values)?;
            }
            Selection::Inline { selections, .. } => substitute_all(selections, values)?,
            Selection::Spread(_) => {}
        }
    }
    Ok(())
}

impl Document {
    /// Substitutes the variables (from `variables`, else defaults) and validates like GitHub: required and
    /// typed variables, declared variables only, defined fragments without cycles, at most `MAX_FIELDS` fields.
    pub(crate) fn resolve(&mut self, variables: &Value) -> Result<(), DocumentError> {
        let mut values = HashMap::new();
        for definition in &self.definitions {
            let value = match variables.get(&definition.name) {
                Some(value) => value.clone(),
                None => definition.default.clone().unwrap_or(Value::Null),
            };
            if !definition.ty.accepts(&value) {
                return Err(DocumentError::Variable {
                    name: definition.name.clone(),
                    ty: definition.ty.to_string(),
                });
            }
            values.insert(definition.name.clone(), value);
        }
        substitute_all(&mut self.selections, &values)?;
        for fragment in self.fragments.values_mut() {
            substitute_all(&mut fragment.selections, &values)?;
        }
        let mut sizes = HashMap::new();
        if self.size(&self.selections, &mut sizes, &mut Vec::new())? > MAX_FIELDS {
            return Err(DocumentError::TooLarge);
        }
        Ok(())
    }

    /// Field selections of `selections` with fragments expanded, saturating; `path` holds the fragments being
    /// expanded (a repeat is a loop), `sizes` the finished ones.
    fn size<'a>(
        &'a self,
        selections: &'a [Selection],
        sizes: &mut HashMap<&'a str, usize>,
        path: &mut Vec<&'a str>,
    ) -> Result<usize, DocumentError> {
        let mut total = 0usize;
        for selection in selections {
            let size = match selection {
                Selection::Field(field) => 1 + self.size(&field.selections, sizes, path)?,
                Selection::Inline { selections, .. } => self.size(selections, sizes, path)?,
                Selection::Spread(name) => {
                    if path.contains(&name.as_str()) {
                        return Err(DocumentError::FragmentLoop { name: name.clone() });
                    }
                    if let Some(size) = sizes.get(name.as_str()) {
                        *size
                    } else {
                        let fragment = self
                            .fragments
                            .get(name)
                            .ok_or_else(|| DocumentError::UnknownFragment { name: name.clone() })?;
                        path.push(name);
                        let size = self.size(&fragment.selections, sizes, path)?;
                        path.pop();
                        sizes.insert(name, size);
                        size
                    }
                }
            };
            total = total.saturating_add(size);
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn field(selection: &Selection) -> &Field {
        match selection {
            Selection::Field(field) => field,
            other => panic!("not a field: {other:?}"),
        }
    }

    fn resolved(src: &str, variables: Value) -> Result<Document, DocumentError> {
        let mut doc = parse(src)?;
        doc.resolve(&variables)?;
        Ok(doc)
    }

    #[test]
    fn aliases_arguments_variables_and_defaults_are_resolved() {
        let doc = resolved(
            r#"query Batch($o: String!, $n: Int = 3) {
                rateLimit { cost remaining resetAt }
                p0: repository(owner: $o, name: "widgets") { pullRequest(number: $n) { title } }
                p1: repository(owner: "acme", name: "x\"y\ud83d\ude00") { issue(number: 7) { milestone { title } } }
            }"#,
            json!({"o": "acme"}),
        )
        .unwrap();
        assert_eq!(doc.kind, Kind::Query);
        assert_eq!(doc.selections.len(), 3);
        let p0 = field(&doc.selections[1]);
        assert_eq!(p0.key(), "p0");
        assert_eq!(p0.args["owner"], json!("acme"));
        assert_eq!(
            field(&p0.selections[0]).args["number"],
            json!(3),
            "the default applies"
        );
        assert_eq!(
            field(&doc.selections[2]).args["name"],
            json!("x\"y\u{1F600}")
        );
    }

    #[test]
    fn variables_are_required_typed_and_declared() {
        let src = "query($o: String!, $n: Int) { a: repository(owner: $o, name: \"p\") { pullRequest(number: $n) { id } } }";
        let missing = DocumentError::Variable {
            name: "o".into(),
            ty: "String!".into(),
        };
        assert_eq!(
            resolved(src, json!({"owner": "acme"})).unwrap_err(),
            missing
        );
        assert_eq!(resolved(src, Value::Null).unwrap_err(), missing);
        assert_eq!(
            resolved(src, json!({"o": "acme", "n": "12"})).unwrap_err(),
            DocumentError::Variable {
                name: "n".into(),
                ty: "Int".into()
            }
        );
        assert!(
            resolved(src, json!({"o": "acme"})).is_ok(),
            "an optional variable may be absent"
        );
        assert_eq!(
            resolved(
                "{ a: repository(owner: $o, name: \"p\") { id } }",
                json!({"o": "acme"})
            )
            .unwrap_err(),
            DocumentError::Undeclared { name: "o".into() }
        );
        assert!(
            resolved("query($s: [PullRequestState!]) { a: repository(owner: \"a\", name: \"b\") { id } }", json!({"s": ["OPEN"]}))
                .is_ok()
        );
    }

    #[test]
    fn enums_lists_fragments_and_inline_fragments_parse() {
        let doc = resolved(
            "{ r: repository(owner:\"a\",name:\"b\") { pullRequests(states:[OPEN], first:100) { nodes { ...Ref } } } }
             fragment Ref on PullRequest { number requestedReviewer { ... on User { login } ... on Team { name } } }",
            Value::Null,
        )
        .unwrap();
        let listing = field(&field(&doc.selections[0]).selections[0]);
        assert_eq!(listing.args["states"], json!(["OPEN"]));
        assert_eq!(listing.args["first"], json!(100));
        assert_eq!(doc.fragments["Ref"].on, "PullRequest");
        assert!(
            matches!(&field(&listing.selections[0]).selections[0], Selection::Spread(name) if name == "Ref")
        );
        assert!(matches!(
            &field(&doc.fragments["Ref"].selections[1]).selections[0],
            Selection::Inline { on: Some(on), .. } if on == "User"
        ));
    }

    #[test]
    fn fragment_loops_unknown_fragments_and_fan_out_are_refused() {
        assert_eq!(
            resolved("{ ...A } fragment A on Query { rateLimit { cost } ...B } fragment B on Query { ...A }", Value::Null)
                .unwrap_err(),
            DocumentError::FragmentLoop { name: "A".into() }
        );
        assert_eq!(
            resolved("{ ...Missing }", Value::Null).unwrap_err(),
            DocumentError::UnknownFragment {
                name: "Missing".into()
            }
        );
        let mut fan_out = String::from("{ ...F0 } fragment F15 on Query { rateLimit { cost } }");
        for level in 0..15 {
            fan_out.push_str(&format!(
                " fragment F{level} on Query {{ {} }}",
                format!("...F{} ", level + 1).repeat(8)
            ));
        }
        assert_eq!(
            resolved(&fan_out, Value::Null).unwrap_err(),
            DocumentError::TooLarge
        );
    }

    #[test]
    fn nesting_is_bounded() {
        let deep = format!(
            "{{ r: repository(owner: \"a\", name: \"b\") {}{} }}",
            "{ a ".repeat(50_000),
            "}".repeat(50_000)
        );
        assert!(matches!(parse(&deep), Err(ParseError::TooDeep { .. })));
        let deep_value = format!("{{ a(b: {}{}) }}", "[".repeat(10_000), "]".repeat(10_000));
        assert!(matches!(
            parse(&deep_value),
            Err(ParseError::TooDeep { .. })
        ));
        let fine = format!("{{ {}x{} }}", "a { ".repeat(60), " }".repeat(60));
        assert!(parse(&fine).is_ok());
    }

    #[test]
    fn mutations_are_recognised_and_errors_are_typed() {
        let doc =
            parse("mutation($in: AddInput!) { addStar(input: $in) { clientMutationId } }").unwrap();
        assert_eq!(doc.kind, Kind::Mutation);
        assert_eq!(parse("{ a(b: ").unwrap_err(), ParseError::End);
        assert_eq!(
            parse("{ a(b: \"x) }").unwrap_err(),
            ParseError::Unterminated { at: 7 }
        );
        assert_eq!(
            parse("{ a(b: \"\\ud83d\") }").unwrap_err(),
            ParseError::Unterminated { at: 7 }
        );
        assert_eq!(
            parse("fragment F on X { a }").unwrap_err(),
            ParseError::NoOperation
        );
        assert!(matches!(
            parse("{ a % }"),
            Err(ParseError::Unexpected { at: 4, .. })
        ));
    }
}
