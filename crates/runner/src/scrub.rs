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

/// A byte of a token-shaped word: token characters and the dots of a JWT.
fn word_char(byte: u8) -> bool {
    token_char(byte) || byte == b'.'
}

/// Whether `run` looks random rather than like a lowercase identifier: it has an upper-case letter or a digit.
fn looks_random(run: &[u8]) -> bool {
    run.iter()
        .any(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// The spans of `word` (bytes for which [`word_char`] holds) to redact. Shapes are found anywhere in the word,
/// not only at its start, so a token glued to other characters (`x<token>`) is still found. Linear: each
/// dot-separated segment is tried once as a JWT start (its first `eyJ`; a later one could only be shorter),
/// and a prefixed token runs to the end of the word, so only the first occurrence of each prefix counts.
fn word_spans(word: &[u8], prefixes: &[&str]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut segments = Vec::new();
    let mut start = 0;
    for (at, byte) in word.iter().enumerate() {
        if *byte == b'.' {
            segments.push((start, at));
            start = at + 1;
        }
    }
    segments.push((start, word.len()));
    let mut index = 0;
    while index + 2 < segments.len() {
        let (begin, header_end) = segments[index];
        let jwt = find(&word[begin..header_end], b"eyJ").map(|offset| begin + offset);
        let (payload_start, payload_end) = segments[index + 1];
        let (signature_start, signature_end) = segments[index + 2];
        match jwt {
            Some(at)
                if header_end - at >= MIN_JWT_SEGMENT
                    && payload_end - payload_start >= MIN_JWT_SEGMENT
                    && signature_end > signature_start =>
            {
                spans.push((at, signature_end));
                index += 3;
            }
            _ => index += 1,
        }
    }
    let prefixed = prefixes
        .iter()
        .filter_map(|prefix| {
            let at = find(word, prefix.as_bytes())?;
            let run = &word[at + prefix.len()..];
            (run.len() >= MIN_PREFIXED_TOKEN_CHARS && looks_random(run)).then_some(at)
        })
        .min();
    if let Some(at) = prefixed {
        spans.push((at, word.len()));
    }
    spans.sort_unstable();
    spans
}

/// The first occurrence of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// `text` with JWTs and prefixed token runs replaced by [`REDACTED`]; linear in the text length for a fixed
/// set of short prefixes.
pub fn token_shapes(text: &str, prefixes: &[&str]) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut at = 0;
    while at < bytes.len() {
        if !word_char(bytes[at]) {
            at += 1;
            continue;
        }
        let end = at + bytes[at..].iter().take_while(|b| word_char(**b)).count();
        for (from, to) in word_spans(&bytes[at..end], prefixes) {
            let (from, to) = (at + from, at + to);
            if from < copied {
                // Overlaps a span already redacted.
                copied = copied.max(to);
                continue;
            }
            out.push_str(&text[copied..from]);
            out.push_str(REDACTED);
            copied = to;
        }
        at = end;
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
    fn tokens_glued_to_preceding_characters_are_redacted() {
        let jwt = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJhY2N0LTEifQ.c2lnbmF0dXJl";
        let refresh = "rt_NEWrotatedRefreshToken0123456789";
        assert_eq!(
            token_shapes(&format!("x{jwt} 9{refresh} key=x{refresh}"), &["rt_"]),
            "x[redacted] 9[redacted] key=x[redacted]"
        );
        // Lowercase identifiers that merely contain a prefix are not tokens.
        let words = "smart-review-codex-staging-directory-name and art_of_the_lowercase_identifier";
        assert_eq!(token_shapes(words, &["codex-", "rt_"]), words);
    }

    #[test]
    fn token_shape_search_stays_linear_on_adversarial_text() {
        let jwt = format!("{0}.{0}.x", "eyJ".repeat(200_000));
        let prefixed = "rt_".repeat(200_000) + "A";
        let text = format!("{jwt} {prefixed} {}", "eyJ.".repeat(200_000));
        let started = std::time::Instant::now();
        assert_eq!(
            token_shapes(&text, &["rt_"]),
            format!("[redacted] [redacted] {}", "eyJ.".repeat(200_000))
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
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
