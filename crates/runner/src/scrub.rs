//! Removal of known secret values from everything that leaves a job (`runner-protocol.md` "Secrets and
//! scrubbing", threat model T10), and the plain-text clipping of event summaries.

use serde_json::Value;

/// Replacement for a scrubbed secret value.
pub const REDACTED: &str = "[redacted]";

/// Longest event summary in characters (`JobEvent.summary`).
pub const SUMMARY_MAX_CHARS: usize = 200;

/// Replaces known secret values with [`REDACTED`]. Each layer scrubs the secrets it holds: `execute` the
/// agent's credentials, the Codex provider its credential copy, the job loop the job token and runner secret.
#[derive(Clone, Default)]
pub struct Scrubber {
    /// Non-empty secrets, longest first so a secret containing another is replaced whole.
    secrets: Vec<String>,
}

impl std::fmt::Debug for Scrubber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scrubber")
            .field("secrets", &self.secrets.len())
            .finish()
    }
}

impl Scrubber {
    /// A scrubber for `secrets`; empty values are ignored.
    pub fn new<'a>(secrets: impl IntoIterator<Item = &'a str>) -> Self {
        let mut secrets: Vec<String> = secrets
            .into_iter()
            .filter(|secret| !secret.is_empty())
            .map(str::to_owned)
            .collect();
        secrets.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        secrets.dedup();
        Self { secrets }
    }

    /// Adds `secrets` (e.g. a credential rotated during the job); empty values are ignored.
    pub fn extend<'a>(&mut self, secrets: impl IntoIterator<Item = &'a str>) {
        let mut all = std::mem::take(&mut self.secrets);
        all.extend(
            secrets
                .into_iter()
                .filter(|secret| !secret.is_empty())
                .map(str::to_owned),
        );
        *self = Self::new(all.iter().map(String::as_str));
    }

    /// The union of two scrubbers.
    pub fn with(&self, other: &Scrubber) -> Scrubber {
        let mut both = self.clone();
        both.extend(other.secrets.iter().map(String::as_str));
        both
    }

    /// [`Scrubber::text`] plus token-shaped strings (for live text under process isolation, where a credential
    /// rotated by the agent is known only after the run): JWTs, and runs of at least
    /// [`MIN_PREFIXED_TOKEN_CHARS`] token characters after a prefix of a known secret (`rt_`, `ghs_`, `sk-`).
    pub fn text_and_token_shapes(&self, text: &str) -> String {
        let prefixes: Vec<&str> = self
            .secrets
            .iter()
            .filter_map(|s| token_prefix(s))
            .collect();
        token_shapes(&self.text(text), &prefixes)
    }

    /// Whether no secret is known.
    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    /// `text` with every known secret replaced.
    pub fn text(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), REDACTED);
            }
        }
        out
    }

    /// `value` with every known secret replaced in strings and object keys.
    pub fn value(&self, value: Value) -> Value {
        if self.secrets.is_empty() {
            return value;
        }
        match value {
            Value::String(text) => Value::String(self.text(&text)),
            Value::Array(items) => Value::Array(items.into_iter().map(|v| self.value(v)).collect()),
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(key, v)| (self.text(&key), self.value(v)))
                    .collect(),
            ),
            other => other,
        }
    }
}

/// `text` as one plain line of at most `max_chars` characters: control characters become spaces, runs of
/// whitespace collapse, and a clipped text ends with `…`.
pub fn clip(text: &str, max_chars: usize) -> String {
    let mut line = String::with_capacity(text.len().min(max_chars * 4));
    let mut last_space = true;
    for ch in text.chars() {
        if ch.is_whitespace() || ch.is_control() {
            if !last_space {
                line.push(' ');
                last_space = true;
            }
        } else {
            line.push(ch);
            last_space = false;
        }
    }
    let trimmed = line.trim_end();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_owned();
    }
    let mut clipped: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn scrubs_every_occurrence_of_every_secret() {
        let scrubber = Scrubber::new(["ghs_canary", "memcan-canary"]);
        let text = scrubber.text("a ghs_canary b memcan-canary c ghs_canary");
        assert_eq!(text, "a [redacted] b [redacted] c [redacted]");
    }

    #[test]
    fn longer_secret_is_replaced_whole() {
        let scrubber = Scrubber::new(["abc", "abcdef"]);
        assert_eq!(scrubber.text("xabcdefx"), "x[redacted]x");
    }

    #[test]
    fn empty_secret_is_ignored() {
        let scrubber = Scrubber::new(["", "s3cret"]);
        assert_eq!(scrubber.text("plain"), "plain");
    }

    #[test]
    fn scrubs_nested_json_strings_and_keys() {
        let scrubber = Scrubber::new(["tok-canary"]);
        let value = json!({"a": ["x tok-canary", {"tok-canary": 1}], "n": 2});
        assert_eq!(
            scrubber.value(value),
            json!({"a": ["x [redacted]", {"[redacted]": 1}], "n": 2})
        );
    }

    #[test]
    fn extended_and_combined_scrubbers_know_every_secret() {
        let mut scrubber = Scrubber::new(["first-secret"]);
        scrubber.extend(["second-secret-longer", "first-secret"]);
        let both = scrubber.with(&Scrubber::new(["third"]));
        assert_eq!(
            both.text("first-secret second-secret-longer third"),
            "[redacted] [redacted] [redacted]"
        );
    }

    #[test]
    fn debug_does_not_show_secrets() {
        let scrubber = Scrubber::new(["tok-canary"]);
        assert!(!format!("{scrubber:?}").contains("tok-canary"));
    }

    #[test]
    fn clip_makes_one_plain_line() {
        assert_eq!(clip("a\nb\t\tc\u{7}d  ", 200), "a b c d");
    }

    #[test]
    fn clip_limits_characters_not_bytes() {
        let text = "ż".repeat(300);
        let clipped = clip(&text, SUMMARY_MAX_CHARS);
        assert_eq!(clipped.chars().count(), SUMMARY_MAX_CHARS);
        assert!(clipped.ends_with('…'));
    }
}

/// Shortest run of token characters after a known prefix that is redacted as a token.
pub const MIN_PREFIXED_TOKEN_CHARS: usize = 20;
/// Shortest JWT segment (header and payload) taken as a token.
const MIN_JWT_SEGMENT: usize = 10;

fn token_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

/// The prefix of a secret up to and including its first `_` or `-` within 8 characters (`rt_…`, `ghs_…`).
fn token_prefix(secret: &str) -> Option<&str> {
    let end = secret
        .bytes()
        .take(8)
        .position(|b| b == b'_' || b == b'-')?;
    (end >= 1).then(|| &secret[..=end])
}

/// Length of a JWT (`eyJ<header>.<payload>.<signature>`) at the start of `text`, if it is one.
fn jwt_len(text: &[u8]) -> Option<usize> {
    if !text.starts_with(b"eyJ") {
        return None;
    }
    let mut at = 0;
    for segment in 0..3 {
        let run = text[at..].iter().take_while(|b| token_char(**b)).count();
        if run < if segment < 2 { MIN_JWT_SEGMENT } else { 1 } {
            return None;
        }
        at += run;
        if segment < 2 {
            if text.get(at) != Some(&b'.') {
                return None;
            }
            at += 1;
        }
    }
    Some(at)
}

/// `text` with JWTs and prefixed token runs replaced by [`REDACTED`]; linear in the text length.
pub fn token_shapes(text: &str, prefixes: &[&str]) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut at = 0;
    while at < bytes.len() {
        let boundary = at == 0 || !token_char(bytes[at - 1]);
        let token = boundary
            .then(|| {
                jwt_len(&bytes[at..]).or_else(|| {
                    prefixes.iter().find_map(|prefix| {
                        let rest = bytes[at..].strip_prefix(prefix.as_bytes())?;
                        let run = rest
                            .iter()
                            .take_while(|b| token_char(**b) || **b == b'.')
                            .count();
                        (run >= MIN_PREFIXED_TOKEN_CHARS).then_some(prefix.len() + run)
                    })
                })
            })
            .flatten();
        match token {
            Some(len) => {
                out.push_str(&text[copied..at]);
                out.push_str(REDACTED);
                at += len;
                copied = at;
            }
            None => at += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
mod token_shape_tests {
    use super::*;

    #[test]
    fn jwts_and_prefixed_tokens_are_redacted_but_hashes_are_not() {
        let jwt = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJhY2N0LTEifQ.c2lnbmF0dXJl";
        let refresh = "rt_NEWrotatedRefreshToken0123456789";
        let text = format!(
            "token {jwt} and {refresh}; commit 3f9c2a7e1b0d4c5a6e7f8091a2b3c4d5e6f70812 ok"
        );
        let scrubbed = token_shapes(&text, &["rt_"]);
        assert_eq!(
            scrubbed,
            "token [redacted] and [redacted]; commit 3f9c2a7e1b0d4c5a6e7f8091a2b3c4d5e6f70812 ok"
        );
        assert_eq!(
            token_shapes("rt_short and eyJx.y.z", &["rt_"]),
            "rt_short and eyJx.y.z"
        );
    }

    #[test]
    fn prefixes_come_from_the_known_secrets() {
        let scrubber = Scrubber::new(["rt_OLDrefreshToken0123456789", "ghs_installationtoken"]);
        let text = "new rt_ROTATEDrefreshTokenABCDEFGHIJ and ghs_ROTATEDinstallationTOKEN0123";
        assert_eq!(
            scrubber.text_and_token_shapes(text),
            "new [redacted] and [redacted]"
        );
        assert_eq!(token_prefix("opaquetoken"), None);
        assert_eq!(token_prefix("sk-abc"), Some("sk-"));
    }
}
