//! Deterministic parsing of thepastaclaw's markers (spike S13-1): the finding key (`finding=`, `dedupe=`), the
//! thread-status block, thread answers, the round phase marker and the gate comment; the normalised finding text
//! and its hash; body findings of a round review. Only `thepastaclaw-*` markers and their known keys are read;
//! every other marker and key is ignored (threat model T24). Callers apply these to bot-authored text only.

use contracts::api::review_loop::BotThreadStatus;
use contracts::common::LineRange;

use crate::secrets::sha256;

const REVIEW_MARKER: &str = "<!-- thepastaclaw-review v1";
const THREAD_STATUS: &str = "<!-- thepastaclaw-thread-status v1 -->";
const REVIEW_STATUS: &str = "<!-- thepastaclaw-review-status v1 -->";
const THREAD_ANSWER: &str = "<!-- thepastaclaw-thread-answer v1";
const PHASE_MARKER: &str = "<!-- thepastaclaw-review-phase v1";
const GATE_MARKER: &str = "<!-- thepastaclaw-gate v1 -->";
/// Characters kept of a title taken from free text.
const TITLE_CHARS: usize = 120;

/// The key markers of a thread's first comment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindingMarker {
    /// `finding=` (12 hex); not unique in the legacy grammar.
    pub finding: Option<String>,
    /// `dedupe=` (16 hex): the stable key when present.
    pub dedupe: Option<String>,
}

/// The `key=value` tokens of the first `marker` comment; callers keep the keys they know and check their values.
fn marker_values<'a>(body: &'a str, marker: &str) -> Option<Vec<(&'a str, &'a str)>> {
    let line = body
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with(marker))?;
    let inner = line.strip_prefix(marker)?.strip_suffix("-->")?;
    Some(
        inner
            .split_whitespace()
            .filter_map(|token| token.split_once('='))
            .collect(),
    )
}

fn is_hex(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The finding marker of a thread's first comment; `None` without a `thepastaclaw-review v1` marker.
pub fn finding_marker(body: &str) -> Option<FindingMarker> {
    let values = marker_values(body, REVIEW_MARKER)?;
    let pick = |key: &str, len: usize| {
        values
            .iter()
            .find(|(name, value)| *name == key && is_hex(value, len))
            .map(|(_, value)| value.to_ascii_lowercase())
    };
    Some(FindingMarker {
        finding: pick("finding", 12),
        dedupe: pick("dedupe", 16),
    })
}

/// A bot answer in a thread (a reply, not an item).
pub fn is_thread_answer(body: &str) -> bool {
    body.trim_start().starts_with(THREAD_ANSWER)
}

/// The head a round review validated: `phase=final sha=<40 hex>` of the phase marker; `None` for other reviews.
pub fn round_sha(body: &str) -> Option<String> {
    let values = marker_values(body, PHASE_MARKER)?;
    let phase_final = values
        .iter()
        .any(|(key, value)| *key == "phase" && *value == "final");
    let sha = values
        .iter()
        .find(|(key, value)| *key == "sha" && is_hex(value, 40))
        .map(|(_, value)| value.to_ascii_lowercase())?;
    phase_final.then_some(sha)
}

/// The bot's own status of a thread, from its status block.
pub fn bot_status(body: &str) -> Option<BotThreadStatus> {
    let mut lines = body.lines().map(str::trim);
    lines.find(|line| *line == THREAD_STATUS)?;
    let quote = lines.find(|line| !line.is_empty())?.strip_prefix('>')?;
    let status = quote.split("**").nth(1)?.trim();
    match status {
        "Resolved" => Some(BotThreadStatus::Resolved),
        "Deferred" => Some(BotThreadStatus::Deferred),
        "Withdrawn" => Some(BotThreadStatus::Withdrawn),
        "No longer applies" => Some(BotThreadStatus::NoLongerApplies),
        "Disputed" => Some(BotThreadStatus::Disputed),
        _ => None,
    }
}

/// The finding text without thepastaclaw's markers and status blocks (spike S13-1 K1): status edits do not change
/// it, the bot rewriting the finding does.
pub fn normalise(body: &str) -> String {
    let mut kept = Vec::new();
    let mut in_block = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed == THREAD_STATUS || trimmed == REVIEW_STATUS {
            in_block = true;
            continue;
        }
        if in_block {
            if trimmed.starts_with('>') {
                continue;
            }
            in_block = false;
        }
        if trimmed.starts_with("<!-- thepastaclaw-") && trimmed.ends_with("-->") {
            continue;
        }
        kept.push(line);
    }
    kept.join("\n").trim().to_owned()
}

/// SHA-256 hex of the normalised text.
pub fn body_hash(normalised: &str) -> String {
    sha256(normalised.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A finding heading `**<emoji> <Severity>: <title>**`: the severity word and the title.
pub fn finding_heading(line: &str) -> Option<(String, String)> {
    let inner = line.trim().strip_prefix("**")?.strip_suffix("**")?;
    let (left, title) = inner.split_once(": ")?;
    let severity = left.split_whitespace().last()?;
    matches!(severity, "Blocking" | "Suggestion" | "Nitpick")
        .then(|| (severity.to_owned(), title.trim().to_owned()))
}

/// The heading of a normalised finding text, if it has one.
pub fn heading_of(text: &str) -> Option<(String, String)> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .and_then(finding_heading)
}

/// A title from free text: the first non-empty line without leading Markdown marks, at most 120 characters.
pub fn title_of(text: &str) -> String {
    let line = text
        .lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(['#', '>', '*', '_', '-', ' '])
        })
        .map(|line| line.trim_end_matches(['*', '_']).trim())
        .find(|line| !line.is_empty())
        .unwrap_or("");
    match line.char_indices().nth(TITLE_CHARS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_owned(),
    }
}

/// `` `path:start-end` `` or `` `path:line` ``.
pub fn location_line(line: &str) -> Option<(String, LineRange)> {
    let inner = line.trim().strip_prefix('`')?.strip_suffix('`')?;
    let (path, range) = inner.rsplit_once(':')?;
    let (start, end) = range.split_once('-').unwrap_or((range, range));
    let (start_line, end_line) = (start.parse().ok()?, end.parse().ok()?);
    let valid = !path.is_empty() && !path.contains(char::is_whitespace) && 0 < start_line;
    (valid && start_line <= end_line).then(|| {
        (
            path.to_owned(),
            LineRange {
                start_line,
                end_line,
            },
        )
    })
}

/// A finding in a round review's body ("not shown inline").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyFinding {
    /// Severity word.
    pub severity: String,
    /// Title.
    pub title: String,
    /// `path:range` when given.
    pub location: Option<(String, LineRange)>,
    /// The finding's text (heading included).
    pub text: String,
}

/// The findings of the "N finding(s) not shown inline" section of a thepastaclaw round review, in order. "Still
/// open from earlier rounds" refers to existing items and follow-up candidates open no thread; neither is read.
pub fn body_findings(body: &str) -> Vec<BodyFinding> {
    let mut findings: Vec<BodyFinding> = Vec::new();
    let mut in_section = false;
    for line in normalise(body).lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("### ") {
            in_section = trimmed.contains("not shown inline");
            continue;
        }
        if trimmed.starts_with("<details") {
            in_section = false;
        }
        if !in_section {
            continue;
        }
        if let Some((severity, title)) = finding_heading(trimmed) {
            findings.push(BodyFinding {
                severity,
                title,
                location: None,
                text: trimmed.to_owned(),
            });
            continue;
        }
        let Some(finding) = findings.last_mut() else {
            continue;
        };
        if finding.location.is_none()
            && finding.text.lines().count() == 1
            && let Some(location) = location_line(trimmed)
        {
            finding.location = Some(location);
        }
        finding.text.push('\n');
        finding.text.push_str(line);
    }
    for finding in &mut findings {
        finding.text = finding.text.trim().to_owned();
    }
    findings
}

/// The state the gate comment reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateState {
    /// `🔍 Review in progress`.
    InProgress,
    /// `⚡ Priority review`: waits for a slot.
    Queued,
    /// `⛔`/`✅ Final review complete`.
    Complete,
}

/// The gate comment's state and the commit it names (8 hex); `None` for any other comment. A status, not an item.
pub fn gate(body: &str) -> Option<(GateState, Option<String>)> {
    let mut lines = body.lines().map(str::trim);
    lines.find(|line| *line == GATE_MARKER)?;
    let status = lines.find(|line| !line.is_empty())?;
    let state = if status.contains("Review in progress") {
        GateState::InProgress
    } else if status.contains("Priority review") {
        GateState::Queued
    } else if status.contains("Final review complete") {
        GateState::Complete
    } else {
        return None;
    };
    let commit = status
        .split("(commit ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .filter(|sha| is_hex(sha, 8))
        .map(str::to_ascii_lowercase);
    Some((state, commit))
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREAD: &str = "<!-- thepastaclaw-review v1 finding=8db55065c705 dedupe=821d7cdc93fec6e1 -->\n\
<!-- thepastaclaw-thread-status v1 -->\n> ✅ **Resolved** at `9f3c2e1a`; see the replies below.\n\n\
**🔴 Blocking: Do not retry a call that may have been applied**\n\nThe retry loop resends.\n\n\
<sub>source: `model-a`</sub>";

    #[test]
    fn finding_markers_read_known_keys_only() {
        assert_eq!(
            finding_marker(THREAD),
            Some(FindingMarker {
                finding: Some("8db55065c705".into()),
                dedupe: Some("821d7cdc93fec6e1".into()),
            })
        );
        let legacy = "<!-- thepastaclaw-review v1 finding=e53a42fd0b67 -->\n**🟡 Suggestion: x**";
        assert_eq!(
            finding_marker(legacy).and_then(|m| m.dedupe),
            None,
            "legacy grammar: finding= only"
        );
        assert_eq!(finding_marker("plain human text"), None);
        assert_eq!(finding_marker("<!-- coderabbit-ai-generated:v1 -->"), None);
    }

    /// T24: unknown keys and malformed values are ignored, never echoed.
    #[test]
    fn markers_parse_only_known_keys() {
        let body = "<!-- thepastaclaw-review v1 finding=c0ffee123456 dedupe=fedcba9876543210 severity=critical \
href=javascript:alert(1) -->\nx";
        assert_eq!(
            finding_marker(body),
            Some(FindingMarker {
                finding: Some("c0ffee123456".into()),
                dedupe: Some("fedcba9876543210".into()),
            })
        );
        let bad = "<!-- thepastaclaw-review v1 finding=../../etc dedupe=<script>x</script> -->";
        assert_eq!(finding_marker(bad), Some(FindingMarker::default()));
        assert_eq!(
            round_sha("<!-- thepastaclaw-review-phase v1 phase=final sha=javascript:1 -->"),
            None
        );
    }

    #[test]
    fn status_block_is_a_fact_and_stripped_from_the_hash() {
        assert_eq!(bot_status(THREAD), Some(BotThreadStatus::Resolved));
        let deferred = THREAD.replace(
            "✅ **Resolved**",
            "⏭️ **Deferred** (maintainer decision; not blocking)",
        );
        assert_eq!(bot_status(&deferred), Some(BotThreadStatus::Deferred));
        let without = THREAD.replace(
            "<!-- thepastaclaw-thread-status v1 -->\n> ✅ **Resolved** at `9f3c2e1a`; see the replies below.\n",
            "",
        );
        assert_eq!(bot_status(&without), None);
        assert_eq!(normalise(THREAD), normalise(&without));
        assert_eq!(
            body_hash(&normalise(THREAD)),
            body_hash(&normalise(&without)),
            "a status edit is not a text edit (S13-1 K1)"
        );
        let rewritten = without.replace("The retry loop resends.", "The loop resends twice.");
        assert_ne!(
            body_hash(&normalise(&without)),
            body_hash(&normalise(&rewritten))
        );
        assert!(!normalise(THREAD).contains("thepastaclaw"));
        assert!(normalise(THREAD).starts_with("**🔴 Blocking"));
    }

    #[test]
    fn headings_titles_and_locations() {
        assert_eq!(
            heading_of(&normalise(THREAD)),
            Some((
                "Blocking".into(),
                "Do not retry a call that may have been applied".into()
            ))
        );
        assert_eq!(finding_heading("**Actionable comments posted: 1**"), None);
        assert_eq!(
            title_of("## Two things\n1. a"),
            "Two things",
            "Markdown marks are dropped"
        );
        assert_eq!(title_of(&"x".repeat(200)).chars().count(), 121);
        assert_eq!(
            location_line("`src/rpc/retry.rs:20-26`"),
            Some((
                "src/rpc/retry.rs".into(),
                LineRange {
                    start_line: 20,
                    end_line: 26
                }
            ))
        );
        assert_eq!(location_line("`a.rs:7`").map(|l| l.1.end_line), Some(7));
        assert_eq!(location_line("`a.rs:9-3`"), None);
        assert_eq!(location_line("plain"), None);
    }

    #[test]
    fn rounds_answers_and_gate() {
        let sha = "52d2da140191a9d47becf6a446310be803c007fc";
        let round = format!(
            "<!-- thepastaclaw-review v1 -->\n<!-- thepastaclaw-review-phase v1 phase=final sha={sha} \
policy=45e4063a411d5823 -->\n## Final"
        );
        assert_eq!(round_sha(&round), Some(sha.to_owned()));
        assert_eq!(
            round_sha(&round.replace("phase=final", "phase=verify")),
            None
        );
        assert_eq!(
            round_sha("<!-- thepastaclaw-review v1 -->\n## Review"),
            None
        );
        assert!(is_thread_answer(
            "<!-- thepastaclaw-thread-answer v1 sha=9f3c reply=None finding=7b44 -->\n**Deferred**"
        ));
        assert!(!is_thread_answer(THREAD));
        assert_eq!(
            gate(
                "<!-- thepastaclaw-gate v1 -->\n⛔ Final review complete — 1 blocking finding(s) (commit 52d2da14) · x"
            ),
            Some((GateState::Complete, Some("52d2da14".into())))
        );
        assert_eq!(
            gate(
                "<!-- thepastaclaw-gate v1 -->\n🔍 Review in progress (commit 52d2da14) · [live](x)"
            )
            .map(|g| g.0),
            Some(GateState::InProgress)
        );
        assert_eq!(gate("Waiting for the author."), None);
    }

    #[test]
    fn body_findings_of_a_round_review() {
        let body = "<!-- thepastaclaw-review v1 -->\n\
<!-- thepastaclaw-review-status v1 -->\n> **Updated after discussion:** x\n> - y\n\n\
Request changes.\n\n### Still open from earlier rounds (1)\n\n\
- 🔴 **Blocking**: Bound — `src/rpc/retry.rs:20-26` (carried)\n\n\
### 1 finding(s) not shown inline (the lines are not part of this PR's diff)\n\n\
**🔴 Blocking: Bound the total retry time**\n`src/rpc/retry.rs:20-26`\n\nEach attempt waits.\n\n\
<sub>source: `model-a`</sub>\n\n<details>\n<summary>Follow-up candidates (optional) (1)</summary>\n\n\
**🟡 Suggestion: Log the final failure once**\n`src/rpc/client.rs:140-142`\n\nx\n</details>";
        let findings = body_findings(body);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].severity, "Blocking");
        assert_eq!(findings[0].title, "Bound the total retry time");
        assert_eq!(
            findings[0].location.as_ref().map(|l| l.0.as_str()),
            Some("src/rpc/retry.rs")
        );
        assert!(findings[0].text.contains("Each attempt waits."));
        assert!(body_findings("Two things before this can merge").is_empty());
    }
}
