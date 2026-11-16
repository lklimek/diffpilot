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
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        secrets.dedup();
        Self { secrets }
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
