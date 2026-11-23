//! Agent-facing text of the `verify_comments` input (plan §4.2): bot comments without HTML, collapsed `<details>`
//! sections and their prompts for AI agents; the PR description without the blocks bots write into it. Pure; stored
//! texts and their hashes are not changed.

/// Bot-written regions of a description, by the start of their first and last line: CodeRabbit's summary and the
/// PR Hygiene status.
const DESCRIPTION_BLOCKS: [(&str, &str); 2] = [
    (
        "<!-- This is an auto-generated comment",
        "<!-- end of auto-generated comment",
    ),
    ("<!-- pr-hygiene:start", "<!-- pr-hygiene:end"),
];
/// The marked region of CodeRabbit's prompt for AI agents.
const AGENT_PROMPT: (&str, &str) = ("<!-- ai-agent-prompt:start", "<!-- ai-agent-prompt:end");
/// HTML elements whose tags are dropped from bot text outside code.
const TAGS: &[&str] = &[
    "a",
    "b",
    "blockquote",
    "br",
    "code",
    "dd",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "kbd",
    "li",
    "ol",
    "p",
    "picture",
    "pre",
    "source",
    "span",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "td",
    "th",
    "tr",
    "ul",
];

/// The description without bot-written blocks and HTML comment lines; `None` when nothing is left.
pub fn description(text: &str) -> Option<String> {
    let mut lines: Vec<&str> = text.lines().collect();
    for (start, end) in DESCRIPTION_BLOCKS {
        lines = without_regions(lines, start, end);
    }
    let kept = lines
        .into_iter()
        .filter(|line| !is_comment_line(line))
        .collect::<Vec<_>>();
    Some(collapse_blank_lines(&kept)).filter(|text| !text.is_empty())
}

/// A bot's comment text without `<details>` sections, prompts for AI agents, HTML comments and tags; code (fenced
/// or inline) is kept as written.
pub fn bot_body(text: &str) -> String {
    let lines = without_regions(text.lines().collect(), AGENT_PROMPT.0, AGENT_PROMPT.1);
    let mut kept = Vec::new();
    let mut fence: Option<&str> = None;
    let mut details = 0usize;
    let mut comment = false;
    let mut prompt_heading = false;
    for line in lines {
        let trimmed = line.trim();
        if let Some(open) = fence {
            if trimmed.starts_with(open) {
                fence = None;
            }
            if details == 0 && !prompt_heading {
                kept.push(line.to_owned());
            } else if fence.is_none() {
                prompt_heading = false;
            }
            continue;
        }
        if let Some(open) = fence_of(trimmed) {
            fence = Some(open);
            if details == 0 && !prompt_heading {
                kept.push(line.to_owned());
            }
            continue;
        }
        if trimmed.starts_with("<details") {
            details += 1;
        }
        if details > 0 {
            if trimmed.contains("</details>") {
                details -= 1;
            }
            continue;
        }
        let (text, open) = without_comments(line, comment);
        comment = open;
        if text.to_ascii_lowercase().contains("prompt for ai agent") {
            prompt_heading = true;
            continue;
        }
        if prompt_heading && !text.trim().is_empty() {
            prompt_heading = false;
        }
        kept.push(without_tags(&text));
    }
    let kept: Vec<&str> = kept.iter().map(String::as_str).collect();
    collapse_blank_lines(&kept)
}

/// `lines` without each complete region from a line starting with `start` to the next line starting with `end`; an
/// unclosed region is kept.
fn without_regions<'a>(lines: Vec<&'a str>, start: &str, end: &str) -> Vec<&'a str> {
    let mut kept = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if line.trim_start().starts_with(start)
            && let Some(close) = lines[index + 1..]
                .iter()
                .position(|line| line.trim_start().starts_with(end))
        {
            index += close + 2;
            continue;
        }
        kept.push(line);
        index += 1;
    }
    kept
}

/// A line that is only an HTML comment.
fn is_comment_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("<!--") && trimmed.ends_with("-->")
}

/// The fence a line opens (three or more backticks or tildes), if any.
fn fence_of(trimmed: &str) -> Option<&'static str> {
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// `line` without HTML comments; `open` says a comment started on an earlier line. Returns the text and whether a
/// comment is still open at its end.
fn without_comments(line: &str, open: bool) -> (String, bool) {
    let mut text = String::new();
    let mut rest = line;
    let mut in_comment = open;
    loop {
        if in_comment {
            let Some(end) = rest.find("-->") else {
                return (text, true);
            };
            rest = &rest[end + 3..];
        }
        let Some(start) = rest.find("<!--") else {
            text.push_str(rest);
            return (text, false);
        };
        text.push_str(&rest[..start]);
        rest = &rest[start + 4..];
        in_comment = true;
    }
}

/// `line` without the tags of [`TAGS`] outside inline code spans.
fn without_tags(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for (index, part) in line.split('`').enumerate() {
        if index > 0 {
            out.push('`');
        }
        if index % 2 == 1 {
            out.push_str(part);
        } else {
            out.push_str(&strip_tags(part));
        }
    }
    out
}

/// `text` without HTML tags whose element is in [`TAGS`].
fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let tag = &rest[start..];
        match tag_len(tag) {
            Some(len) => rest = &tag[len..],
            None => {
                out.push('<');
                rest = &tag[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The length of the HTML tag `text` starts with, when its element is in [`TAGS`].
fn tag_len(text: &str) -> Option<usize> {
    let end = text.find('>')?;
    let inner = text[1..end].trim_start_matches('/').trim_end_matches('/');
    let name = inner
        .split(|c: char| c.is_whitespace())
        .next()
        .unwrap_or_default();
    let attributes = &inner[name.len()..];
    let plain = attributes.is_empty() || attributes.starts_with(char::is_whitespace);
    (plain && TAGS.contains(&name) && !text[1..end].contains('<')).then_some(end + 1)
}

/// The lines joined, trailing spaces dropped, runs of blank lines as one, trimmed.
fn collapse_blank_lines(lines: &[&str]) -> String {
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    for line in lines {
        let line = line.trim_end();
        if line.is_empty() && out.last().is_none_or(|last| last.is_empty()) {
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(|last| last.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CodeRabbit thread comment as dash-evo-tool#1102 had them (spike S13-3), shortened.
    const CODERABBIT: &str = "<!-- coderabbit-ai-generated:v1 -->\n\n\
**<picture><img src=\"https://example.test/amber-16.svg\" width=\"16\" height=\"16\" align=\"absmiddle\" alt=\"\"></picture> Minor** · **Functional Correctness** · **Quick win**\n\n\
**Document the signing-key limitation.**\n\n\
Identities with `ECDSA_HASH160` keys cannot publish; `Option<Key>` stays empty.\n\n\
<!-- ai-agent-prompt:start -->\n\n<details>\n<summary><picture><source media=\"(prefers-color-scheme: dark)\" srcset=\"x\"></picture> Prompt for AI Agents</summary>\n<dl>\n<dd>\n\n\
```\nVerify each finding against current code.\n```\n\n</dd>\n</dl>\n</details>\n\n<!-- ai-agent-prompt:end -->\n\n\
<!-- cr-comment:v1:0daad0027feb753907c5fe46 -->\n\n<!-- This is an auto-generated reply by CodeRabbit -->";

    #[test]
    fn bot_body_drops_markup_details_and_agent_prompts_but_keeps_code() {
        assert_eq!(
            bot_body(CODERABBIT),
            "** Minor** · **Functional Correctness** · **Quick win**\n\n\
**Document the signing-key limitation.**\n\n\
Identities with `ECDSA_HASH160` keys cannot publish; `Option<Key>` stays empty."
        );
        let fenced = "Call it so:\n```rust\nlet a: Vec<b> = <br>;\n```\n<sub>source: `x`</sub>";
        assert_eq!(
            bot_body(fenced),
            "Call it so:\n```rust\nlet a: Vec<b> = <br>;\n```\nsource: `x`"
        );
        let bare = "**Prompt for AI Agents**\n```\nFix it.\n```\nThe loop never ends.";
        assert_eq!(bot_body(bare), "The loop never ends.");
        assert_eq!(
            bot_body("Generic `Option<A>` and Vec<T> stay; <b>bold</b> <!-- x\nmore --> tail"),
            "Generic `Option<A>` and Vec<T> stay; bold\n tail"
        );
        assert_eq!(
            bot_body("<details>\n<summary>x</summary>\nnever closed"),
            ""
        );
    }

    /// dash-evo-tool#1102: CodeRabbit's summary and the PR Hygiene block were 2 KB of its description.
    #[test]
    fn description_drops_bot_blocks_and_comment_lines() {
        let text = "**TL;DR:** Contact details in the DashPay format.\n\n## Testing\n- 490 passed.\n\n\
<!-- pr-hygiene:start -->\n# PR Hygiene · `171e479`\n- [ ] Build failed\n<!-- pr-hygiene:end -->\n\n\n\n\
<!-- This is an auto-generated comment: release notes by coderabbit.ai -->\n<!-- coderabbit-ai-generated:v1 -->\n\n\
## Summary by CodeRabbit\n\n* **New Features**\n<!-- end of auto-generated comment: release notes by coderabbit.ai -->";
        assert_eq!(
            description(text).as_deref(),
            Some("**TL;DR:** Contact details in the DashPay format.\n\n## Testing\n- 490 passed.")
        );
        let unclosed = "Intro.\n<!-- pr-hygiene:start -->\nHand-written text after a stray marker.";
        assert_eq!(
            description(unclosed).as_deref(),
            Some("Intro.\nHand-written text after a stray marker."),
            "an unclosed block is the author's text"
        );
        assert_eq!(
            description("<!-- pr-hygiene:start -->\nx\n<!-- pr-hygiene:end -->"),
            None
        );
    }
}
