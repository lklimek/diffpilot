//! Agent-facing text of the `verify_comments` input (plan §4.2): bot comments without HTML, collapsed `<details>`
//! sections (a suggested fix keeps its summary and code) and their prompts for AI agents; the PR description without
//! the blocks bots write into it. Pure; stored texts and their hashes are not changed.

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
/// Summaries of `<details>` sections that hold a reviewer's code change, lowercase.
const FIX_SUMMARIES: &[&str] = &[
    "suggested fix",
    "suggested change",
    "suggested refactor",
    "committable suggestion",
    "proposed fix",
    "proposed change",
    "proposed refactor",
];
/// Fence info strings of a code change.
const FIX_FENCES: &[&str] = &["diff", "suggestion"];
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
/// or inline) is kept as written. A section that holds a suggested fix keeps its summary and code ([`fix_section`]).
pub fn bot_body(text: &str) -> String {
    let lines = without_regions(text.lines().collect(), AGENT_PROMPT.0, AGENT_PROMPT.1);
    let mut kept = Vec::new();
    let mut comment = false;
    let mut prompt_heading = false;
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim();
        if let Some(open) = fence_of(trimmed) {
            let end = fence_end(&lines, index, open);
            if !prompt_heading {
                kept.extend(lines[index..=end].iter().map(|line| (*line).to_owned()));
            }
            prompt_heading = false;
            index = end + 1;
            continue;
        }
        if trimmed.starts_with("<details") {
            // An unclosed section hides the rest of the comment.
            let Some(end) = details_end(&lines, index) else {
                break;
            };
            kept.extend(fix_section(&lines[index..=end]));
            index = end + 1;
            continue;
        }
        let (text, open) = without_comments(lines[index], comment);
        comment = open;
        index += 1;
        if is_agent_prompt_heading(&text) {
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

/// A line that is only the heading of a prompt for AI agents ("**Prompt for AI Agents**", "## 🤖 Prompt for AI
/// agents:"), whose code block follows it.
fn is_agent_prompt_heading(line: &str) -> bool {
    let words: Vec<String> = strip_tags(line)
        .split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_alphabetic())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect();
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    matches!(
        words.as_slice(),
        ["prompt", "for", "ai", "agent" | "agents"]
    )
}

/// The index of the line that closes the fence `lines[start]` opens with `open`; the last line when it is unclosed.
fn fence_end(lines: &[&str], start: usize, open: &str) -> usize {
    lines[start + 1..]
        .iter()
        .position(|line| line.trim().starts_with(open))
        .map_or(lines.len() - 1, |close| start + 1 + close)
}

/// The index of the line that closes the `<details>` section `lines[start]` opens, nested sections and code
/// included; `None` when it is unclosed.
fn details_end(lines: &[&str], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut index = start;
    while index < lines.len() {
        let trimmed = lines[index].trim();
        if let Some(open) = fence_of(trimmed) {
            index = fence_end(lines, index, open) + 1;
            continue;
        }
        if trimmed.starts_with("<details") {
            depth += 1;
        }
        depth = depth.saturating_sub(trimmed.matches("</details>").count());
        if depth == 0 {
            return Some(index);
        }
        index += 1;
    }
    None
}

/// What a complete `<details>` section leaves: nothing, unless its summary names a suggested fix or it holds a
/// `diff` or `suggestion` fence; then its summary without tags and its code blocks as written, nested sections by
/// the same rule. Other text and HTML are dropped.
fn fix_section(section: &[&str]) -> Vec<String> {
    let (summary, body) = summary_of(section);
    let mut code = Vec::new();
    let mut fix_fence = false;
    let mut index = 0;
    while index < body.len() {
        let trimmed = body[index].trim();
        if let Some(open) = fence_of(trimmed) {
            let info = trimmed.trim_start_matches(['`', '~']).trim();
            fix_fence |= FIX_FENCES.iter().any(|fence| info.starts_with(fence));
            let end = fence_end(body, index, open);
            code.extend(body[index..=end].iter().map(|line| (*line).to_owned()));
            index = end + 1;
        } else if trimmed.starts_with("<details")
            && let Some(end) = details_end(body, index)
        {
            code.extend(fix_section(&body[index..=end]));
            index = end + 1;
        } else {
            index += 1;
        }
    }
    let lower = summary.to_lowercase();
    let fix_summary = FIX_SUMMARIES.iter().any(|name| lower.contains(name));
    if !fix_summary && !fix_fence {
        return Vec::new();
    }
    std::iter::once(summary)
        .filter(|summary| !summary.is_empty())
        .chain(code)
        .collect()
}

/// The section's summary without tags and the lines between it and the closing line. A summary is looked for only
/// before the first nested section or code block.
fn summary_of<'a, 'b>(section: &'a [&'b str]) -> (String, &'a [&'b str]) {
    let inner = &section[..section.len() - 1];
    let head = inner
        .iter()
        .enumerate()
        .take_while(|(index, line)| {
            let trimmed = line.trim();
            *index == 0 || !(trimmed.starts_with("<details") || fence_of(trimmed).is_some())
        })
        .find(|(_, line)| line.contains("</summary>"))
        .map(|(index, _)| index);
    let Some(head) = head else {
        return (String::new(), inner.get(1..).unwrap_or_default());
    };
    let text = section[..=head].join("\n");
    let summary = text
        .find("<summary")
        .and_then(|start| text[start..].find('>').map(|open| start + open + 1))
        .and_then(|from| {
            text[from..]
                .find("</summary>")
                .map(|len| &text[from..from + len])
        })
        .unwrap_or_default();
    (
        without_tags(summary)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        &inner[head + 1..],
    )
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

/// `line` without HTML comments outside inline code spans; `open` says a comment started on an earlier line. Returns
/// the text and whether a comment is still open at its end.
fn without_comments(line: &str, open: bool) -> (String, bool) {
    let mut text = String::with_capacity(line.len());
    let mut rest = line;
    let mut in_comment = open;
    loop {
        if in_comment {
            let Some(end) = rest.find("-->") else {
                return (text, true);
            };
            rest = &rest[end + 3..];
            in_comment = false;
        }
        let Some(at) = rest.find(['`', '<']) else {
            text.push_str(rest);
            return (text, false);
        };
        text.push_str(&rest[..at]);
        rest = &rest[at..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after;
            in_comment = true;
        } else if let Some(after) = rest.strip_prefix('<') {
            text.push('<');
            rest = after;
        } else {
            // A backtick run opens a code span when a run of the same length closes it; else it is literal.
            let ticks = rest.len() - rest.trim_start_matches('`').len();
            let span = code_span_len(&rest[ticks..], ticks).map_or(ticks, |len| ticks + len);
            text.push_str(&rest[..span]);
            rest = &rest[span..];
        }
    }
}

/// The length of `text` up to and including the first run of exactly `ticks` backticks, if any.
fn code_span_len(text: &str, ticks: usize) -> Option<usize> {
    let mut offset = 0;
    while let Some(start) = text[offset..].find('`') {
        let start = offset + start;
        let run = text[start..].len() - text[start..].trim_start_matches('`').len();
        if run == ticks {
            return Some(start + run);
        }
        offset = start + run;
    }
    None
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

    /// Review finding: code spans are kept as written, comment markers in them included, and only a line that is the
    /// "Prompt for AI agents" heading (bold, emoji, heading marks) drops the code after it.
    #[test]
    fn bot_body_keeps_code_spans_and_prose_about_agent_prompts() {
        for text in [
            "The marker `<!-- gate v1 -->` is missing.",
            "Match on `<!--` first.\nThen the next line stays.",
            "The prompt for AI agents in `prompts/ask.md` omits the rule:\n```\nrule 6\n```",
        ] {
            assert_eq!(bot_body(text), text);
        }
        assert_eq!(
            bot_body("## 🤖 Prompt for AI Agents:\n\n```\nFix it.\n```\nThe loop never ends."),
            "The loop never ends."
        );
    }

    /// Review finding: a suggested fix is the reviewer's code. Its `<details>` block keeps its summary and code without
    /// the HTML; other blocks (analysis chains, agent prompts) are dropped. Layout of CodeRabbit's on dash-evo-tool#1158.
    #[test]
    fn bot_body_keeps_the_code_of_suggested_fixes() {
        let text = "Pin the revision.\n\n<details>\n<summary>Suggested fix</summary>\n<dl>\n<dd>\n<br>\n\n\
```diff\n-rev = \"a\"\n+rev = \"b\"\n```\n\n</dd>\n</dl>\n</details>\n\n<!-- suggestion_start -->\n\n\
<details>\n<summary>📝 Committable suggestion</summary>\n<dl>\n<dd>\n<br>\n\n> ‼️ **IMPORTANT**\n\
> Review the replacement and indentation.\n\n```suggestion\nrev = \"b\"\n```\n\n</dd>\n</dl>\n</details>\n\n\
<!-- suggestion_end -->\n\n<details>\n<summary>🧩 Analysis chain</summary>\n\n```shell\nrg rev\n```\n</details>\n\n\
<details><summary>Details</summary>\n\n```diff\n+x\n```\n</details>";
        assert_eq!(
            bot_body(text),
            "Pin the revision.\n\nSuggested fix\n```diff\n-rev = \"a\"\n+rev = \"b\"\n```\n\n📝 Committable suggestion\n\
```suggestion\nrev = \"b\"\n```\n\nDetails\n```diff\n+x\n```"
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
