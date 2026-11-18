//! A small GraphQL document parser for the dashboard documents: operations, fragments, inline fragments and
//! fields with aliases and arguments. Variables are resolved while parsing; directives and variable
//! definitions are skipped.

use std::collections::HashMap;

use serde_json::{Map, Number, Value};

/// The document is not GraphQL the fake understands.
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
    /// A string literal without its closing quote.
    #[error("unterminated string at byte {at} of the GraphQL document")]
    Unterminated {
        /// Byte offset of the opening quote.
        at: usize,
    },
    /// Only fragments, no operation.
    #[error("the GraphQL document has no operation")]
    NoOperation,
}

/// Operation type of the document's first operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Query,
    Mutation,
    Subscription,
}

/// A parsed document: its first operation and every fragment definition.
#[derive(Debug)]
pub(crate) struct Document {
    pub(crate) kind: Kind,
    pub(crate) selections: Vec<Selection>,
    pub(crate) fragments: HashMap<String, Fragment>,
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

/// A field with its resolved arguments.
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
                    let hex: String = (0..4)
                        .filter_map(|_| chars.next().map(|(_, h)| h))
                        .collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

struct Parser<'a> {
    tokens: Vec<(usize, Token)>,
    pos: usize,
    variables: &'a Value,
}

impl Parser<'_> {
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

    /// Skips a balanced `( … )` group (variable definitions).
    fn skip_parens(&mut self) -> Result<(), ParseError> {
        let mut depth = 0usize;
        loop {
            match self.next()? {
                Token::Punct('(') => depth += 1,
                Token::Punct(')') => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
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
                    operation.get_or_insert((Kind::Query, selections));
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
                    if self.peek() == Some(&Token::Punct('(')) {
                        self.skip_parens()?;
                    }
                    self.directives()?;
                    let selections = self.selection_set()?;
                    operation.get_or_insert((kind, selections));
                }
                _ => return Err(self.unexpected()),
            }
        }
        let (kind, selections) = operation.ok_or(ParseError::NoOperation)?;
        Ok(Document {
            kind,
            selections,
            fragments,
        })
    }

    fn selection_set(&mut self) -> Result<Vec<Selection>, ParseError> {
        self.expect('{')?;
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
        match self.next()? {
            Token::Punct('$') => {
                let name = self.name()?;
                Ok(self.variables.get(&name).cloned().unwrap_or(Value::Null))
            }
            Token::Str(text) => Ok(Value::String(text)),
            Token::Num(text) => Ok(text
                .parse::<i64>()
                .map(Value::from)
                .ok()
                .or_else(|| {
                    text.parse::<f64>()
                        .ok()
                        .and_then(Number::from_f64)
                        .map(Value::Number)
                })
                .unwrap_or(Value::String(text))),
            Token::Name(word) => Ok(match word.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                "null" => Value::Null,
                _ => Value::String(word),
            }),
            Token::Punct('[') => {
                let mut items = Vec::new();
                while !self.eat(']') {
                    items.push(self.value()?);
                }
                Ok(Value::Array(items))
            }
            Token::Punct('{') => {
                let mut object = Map::new();
                while !self.eat('}') {
                    let key = self.name()?;
                    self.expect(':')?;
                    object.insert(key, self.value()?);
                }
                Ok(Value::Object(object))
            }
            _ => {
                self.pos -= 1;
                Err(self.unexpected())
            }
        }
    }
}

/// Parses `src`, resolving `$variables` from `variables` (missing ones are `null`).
pub(crate) fn parse(src: &str, variables: &Value) -> Result<Document, ParseError> {
    Parser {
        tokens: tokenize(src)?,
        pos: 0,
        variables,
    }
    .document()
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

    #[test]
    fn aliases_arguments_and_variables_are_resolved() {
        let doc = parse(
            r#"query Batch($o: String!, $n: Int = 3) {
                rateLimit { cost remaining resetAt }
                p0: repository(owner: $o, name: "widgets") { pullRequest(number: $n) { title } }
                p1: repository(owner: "acme", name: "x\"y") { issue(number: 7) { milestone { title } } }
            }"#,
            &json!({"o": "acme", "n": 12}),
        )
        .unwrap();
        assert_eq!(doc.kind, Kind::Query);
        assert_eq!(doc.selections.len(), 3);
        let p0 = field(&doc.selections[1]);
        assert_eq!(p0.key(), "p0");
        assert_eq!(p0.name, "repository");
        assert_eq!(p0.args["owner"], json!("acme"));
        assert_eq!(p0.args["name"], json!("widgets"));
        assert_eq!(field(&p0.selections[0]).args["number"], json!(12));
        assert_eq!(field(&doc.selections[2]).args["name"], json!("x\"y"));
    }

    #[test]
    fn enums_lists_fragments_and_inline_fragments_parse() {
        let doc = parse(
            "{ r: repository(owner:\"a\",name:\"b\") { pullRequests(states:[OPEN], first:100) { nodes { ...Ref } } } }
             fragment Ref on PullRequest { number requestedReviewer { ... on User { login } ... on Team { name } } }",
            &Value::Null,
        )
        .unwrap();
        let listing = field(&field(&doc.selections[0]).selections[0]);
        assert_eq!(listing.args["states"], json!(["OPEN"]));
        assert_eq!(listing.args["first"], json!(100));
        assert_eq!(doc.fragments["Ref"].on, "PullRequest");
        assert!(matches!(
            &field(&doc.fragments["Ref"].selections[1]).selections[0],
            Selection::Inline { on: Some(on), .. } if on == "User"
        ));
    }

    #[test]
    fn mutations_are_recognised_and_errors_are_typed() {
        let doc = parse(
            "mutation { addStar(input: {starrableId: \"x\"}) { clientMutationId } }",
            &Value::Null,
        )
        .unwrap();
        assert_eq!(doc.kind, Kind::Mutation);
        assert_eq!(parse("{ a(b: ", &Value::Null).unwrap_err(), ParseError::End);
        assert_eq!(
            parse("{ a(b: \"x) }", &Value::Null).unwrap_err(),
            ParseError::Unterminated { at: 7 }
        );
        assert_eq!(
            parse("fragment F on X { a }", &Value::Null).unwrap_err(),
            ParseError::NoOperation
        );
        assert!(matches!(
            parse("{ a % }", &Value::Null),
            Err(ParseError::Unexpected { at: 4, .. })
        ));
    }
}
