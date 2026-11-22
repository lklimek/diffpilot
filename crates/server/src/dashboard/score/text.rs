//! Text rules of the skill's regular expressions, matched by hand (ASCII case-insensitive keywords, Unicode word
//! boundaries as in Python `re`).

use std::cmp::Ordering;

/// Python `\w`.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `chars[at..]` starts with the ASCII `word`, ignoring ASCII case.
fn starts_with_at(chars: &[char], at: usize, word: &str) -> bool {
    let mut i = at;
    for w in word.chars() {
        match chars.get(i) {
            Some(c) if c.to_ascii_lowercase() == w => i += 1,
            _ => return false,
        }
    }
    true
}

fn boundary_before(chars: &[char], at: usize) -> bool {
    at == 0 || chars.get(at - 1).is_none_or(|c| !is_word(*c))
}

fn boundary_after(chars: &[char], end: usize) -> bool {
    chars.get(end).is_none_or(|c| !is_word(*c))
}

/// `\b<word>\b`, ignoring ASCII case (skill `BLOCKED_LABEL_RE` with `blocked`).
pub(super) fn has_word(text: &str, word: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let len = word.chars().count();
    (0..chars.len()).any(|i| {
        boundary_before(&chars, i)
            && starts_with_at(&chars, i, word)
            && boundary_after(&chars, i + len)
    })
}

/// Fix or bug wording (skill `FIX_RE`: a word starting with one of these stems).
pub(super) fn has_fix_wording(text: &str) -> bool {
    const STEMS: [&str; 11] = [
        "fix", "bug", "panic", "crash", "fail", "broken", "regress", "leak", "corrupt", "race",
        "deadlock",
    ];
    let chars: Vec<char> = text.chars().collect();
    (0..chars.len())
        .any(|i| boundary_before(&chars, i) && STEMS.iter().any(|s| starts_with_at(&chars, i, s)))
}

/// "blocked by", "blocked on" or "depends on" as words (skill `BLOCKED_TEXT_RE`).
pub(super) fn has_blocker_wording(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let phrase_at = |i: usize, first: &str, seconds: &[&str]| {
        if !starts_with_at(&chars, i, first) {
            return false;
        }
        let mut j = i + first.len();
        let gap = j;
        while chars.get(j).is_some_and(|c| c.is_whitespace()) {
            j += 1;
        }
        j > gap
            && seconds
                .iter()
                .any(|s| starts_with_at(&chars, j, s) && boundary_after(&chars, j + s.len()))
    };
    (0..chars.len()).any(|i| {
        boundary_before(&chars, i)
            && (phrase_at(i, "blocked", &["by", "on"]) || phrase_at(i, "depends", &["on"]))
    })
}

/// Conventional-commit type of a title, lowercased (skill `CONVENTIONAL_RE`), and its `!` breaking mark.
pub(super) fn conventional_type(title: &str) -> Option<(String, bool)> {
    let rest = title.trim_start();
    let word_len: usize = rest
        .chars()
        .take_while(|c| is_word(*c))
        .map(char::len_utf8)
        .sum();
    if word_len == 0 {
        return None;
    }
    let (word, mut rest) = rest.split_at(word_len);
    if let Some(scope) = rest.strip_prefix('(') {
        rest = &scope[scope.find(')')? + 1..];
    }
    let breaking = rest.starts_with('!');
    if breaking {
        rest = &rest[1..];
    }
    rest.trim_start()
        .starts_with(':')
        .then(|| (word.to_lowercase(), breaking))
}

/// A version `v1.8`, `1.8.0`, `v1.8.2+meta`, or (milestones only) `v1.8.x` = 1.8.0 (skill `_version`).
pub(super) fn version(text: &str, milestone: bool) -> Option<(u64, u64, u64)> {
    let mut rest = text.trim();
    rest = rest.strip_prefix(['v', 'V']).unwrap_or(rest);
    let number = |s: &str| -> Option<(u64, usize)> {
        let len = s.bytes().take_while(u8::is_ascii_digit).count();
        if len == 0 {
            return None;
        }
        s[..len].parse().ok().map(|n| (n, len))
    };
    let (major, len) = number(rest)?;
    rest = rest[len..].strip_prefix('.')?;
    let (minor, len) = number(rest)?;
    rest = &rest[len..];
    let mut patch = 0;
    if let Some(after) = rest.strip_prefix('.') {
        if let Some((n, len)) = number(after) {
            patch = n;
            rest = &after[len..];
        } else if milestone && after.starts_with(['x', 'X']) {
            rest = &after[1..];
        } else {
            return None;
        }
    }
    if let Some(meta) = rest.strip_prefix('+') {
        let valid = !meta.is_empty() && meta.chars().all(|c| is_word(c) || c == '.' || c == '-');
        return valid.then_some((major, minor, patch));
    }
    rest.is_empty().then_some((major, minor, patch))
}

/// A built-in ignored check (K12, K17 R4): an approval, policy or review-bot gate (skill `POLICY_CHECK_RE`) or
/// thepastaclaw's "PastaClaw review", used while a repository row lists no Ignored checks.
pub(super) fn is_builtin_ignored_check(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let policy_prefix = name
        .strip_prefix("policy")
        .is_some_and(|rest| rest.chars().next().is_none_or(|c| !is_word(c)));
    name.contains("hygiene")
        || policy_prefix
        || name == "pr title"
        || name.contains("semantic-pull-request")
        || name.contains("milestone")
        || name.contains("coderabbit")
        || name == "review"
        || name == "pastaclaw review"
}

/// Natural order of display ids (skill `_id_key`): prefix, then a numeric suffix before any other suffix.
pub(super) fn compare_ids(a: &str, b: &str) -> Ordering {
    fn key(id: &str) -> (&str, u8, u64, &str) {
        let (prefix, rest) = id
            .rfind(['-', '#'])
            .map_or(("", id), |at| (&id[..at], &id[at + 1..]));
        match rest.parse::<u64>() {
            Ok(n) if rest.bytes().all(|b| b.is_ascii_digit()) => (prefix, 0, n, ""),
            _ => (prefix, 1, 0, rest),
        }
    }
    key(a).cmp(&key(b))
}
