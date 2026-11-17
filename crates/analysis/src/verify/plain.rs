//! Plain-text check of an answer shown to the reviewer: no HTML-like tag outside code, and no link other than
//! `https://github.com/…` (schemes and hosts compared case-insensitively, after a URL parse).

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

/// Why a text is not plain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Unplain {
    /// An HTML-like tag outside code.
    Markup,
    /// A link that is not `https://github.com/…`.
    Link,
    /// A pattern did not compile; nothing is shown unchecked.
    Unchecked,
}

impl Unplain {
    /// Plain description for the repair prompt.
    pub(super) fn describe(self) -> &'static str {
        match self {
            Self::Markup => "HTML markup",
            Self::Link => "a link outside https://github.com/",
            Self::Unchecked => "text that could not be checked",
        }
    }
}

struct Patterns {
    /// An absolute URL (`scheme://…`), checked anywhere, code included.
    absolute: Regex,
    /// A script or data URL, checked anywhere.
    script: Regex,
    /// An autolink `<scheme:…>`.
    autolink: Regex,
    /// `<` followed by a letter, `/` or `!`.
    tag: Regex,
    /// A Markdown link or image target, or a reference definition.
    target: Regex,
    /// A scheme-relative URL (`//host…`).
    relative: Regex,
}

static PATTERNS: LazyLock<Option<Patterns>> = LazyLock::new(|| {
    Some(Patterns {
        absolute: Regex::new(r#"(?i)\b[a-z][a-z0-9+.-]*://[^\s<>"'`()\[\]]*"#).ok()?,
        script: Regex::new(r"(?i)\b(?:javascript|vbscript):|\bdata:[a-z]+/").ok()?,
        autolink: Regex::new(r"<([A-Za-z][A-Za-z0-9+.-]*:[^\s<>]*)>").ok()?,
        tag: Regex::new(r"<[A-Za-z/!]").ok()?,
        target: Regex::new(r"(?m)\]\(\s*<?([^\s)>]*)|^\s*\[[^\]]+\]:\s*<?([^\s>]*)").ok()?,
        relative: Regex::new(r#"(?:^|[\s(\[<"'=])//[A-Za-z0-9]"#).ok()?,
    })
});

/// Why `text` is not plain text, if it is not.
pub(super) fn check(text: &str) -> Option<Unplain> {
    let Some(patterns) = PATTERNS.as_ref() else {
        return Some(Unplain::Unchecked);
    };
    let foreign = |found: regex::Match<'_>| !github(found.as_str());
    if patterns.absolute.find_iter(text).any(foreign) || patterns.script.is_match(text) {
        return Some(Unplain::Link);
    }
    let prose = without_code(text);
    for captures in patterns.autolink.captures_iter(&prose) {
        if captures.get(1).is_some_and(foreign) {
            return Some(Unplain::Link);
        }
    }
    let prose = patterns.autolink.replace_all(&prose, " ");
    if patterns.tag.is_match(&prose) {
        return Some(Unplain::Markup);
    }
    let targets = patterns.target.captures_iter(&prose);
    if targets
        .filter_map(|captures| captures.get(1).or_else(|| captures.get(2)))
        .any(foreign)
        || patterns.relative.is_match(&prose)
    {
        return Some(Unplain::Link);
    }
    None
}

/// `https://github.com/…` without user info or port.
fn github(link: &str) -> bool {
    Url::parse(link).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("github.com")
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
    })
}

/// `text` with closed fenced code blocks and inline code spans replaced by a space; an unclosed fence is prose.
fn without_code(text: &str) -> String {
    let mut prose = String::with_capacity(text.len());
    let mut fenced: Option<((char, usize), Vec<&str>)> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let opener = ['`', '~'].into_iter().find_map(|mark| {
            let count = trimmed.chars().take_while(|c| *c == mark).count();
            (count >= 3).then_some((mark, count))
        });
        match (&mut fenced, opener) {
            (None, Some(opened)) => fenced = Some((opened, vec![line])),
            (Some(((mark, count), _)), Some((found, length)))
                if found == *mark && length >= *count =>
            {
                fenced = None;
                prose.push(' ');
            }
            (Some((_, lines)), _) => lines.push(line),
            (None, None) => prose.push_str(&without_spans(line)),
        }
    }
    for line in fenced.map(|(_, lines)| lines).unwrap_or_default() {
        prose.push_str(&without_spans(line));
    }
    prose
}

/// `line` with inline code spans (a backtick run closed by a run of the same length) replaced by a space.
fn without_spans(line: &str) -> String {
    let mut prose = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        let run = rest[start..].chars().take_while(|c| *c == '`').count();
        let after = &rest[start + run..];
        let close = (0..after.len())
            .filter(|&at| after.is_char_boundary(at) && after[at..].starts_with('`'))
            .find(|&at| {
                let length = after[at..].chars().take_while(|c| *c == '`').count();
                length == run && (at == 0 || !after[..at].ends_with('`'))
            });
        prose.push_str(&rest[..start]);
        match close {
            Some(at) => {
                prose.push(' ');
                rest = &after[at + run..];
            }
            None => {
                prose.push_str(&rest[start..start + run]);
                rest = after;
            }
        }
    }
    prose.push_str(rest);
    prose
}
