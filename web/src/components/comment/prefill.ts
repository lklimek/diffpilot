// Draft review comments made from LLM text ("Use as review comment"): plain text, marked as LLM-suggested.
import type { Annotation } from "../../api/generated/types";

/** First line of every prefilled draft; the user may delete it before posting. */
export const LLM_SUGGESTED = "LLM-suggested — review before posting";

const suggested = (body: string) => `${LLM_SUGGESTED}\n\n${forGithub(body.trim())}`;

// GitHub turns `@login` into a notification and `#123` / `owner/repo#123` into a cross-reference; as code they
// stay plain text. Code spans, fenced blocks and URLs are left as written.
const REFERENCE = /(?<![\w`/.@#-])(?:@[A-Za-z0-9][\w-]*(?:\/[\w.-]+)?|(?:[\w.-]+\/[\w.-]+)?#\d+\b)/g;
const KEPT = /(`+)[^`]*?\1|\bhttps?:\/\/\S+/g;
const FENCE = /^ {0,3}(`{3,}|~{3,})/;

const quoteReferences = (text: string) => text.replace(REFERENCE, (token) => `\`${token}\``);

function forGithubLine(line: string): string {
  let out = "";
  let at = 0;
  for (const kept of line.matchAll(KEPT)) {
    out += quoteReferences(line.slice(at, kept.index)) + kept[0];
    at = kept.index + kept[0].length;
  }
  out += quoteReferences(line.slice(at));
  // A line starting with `#` would be a heading.
  return out.replace(/^( {0,3})#/, "$1\\#");
}

/** `text` safe to post as GitHub Markdown: no mentions, cross-references or headings. */
function forGithub(text: string): string {
  let fence: string | null = null;
  return text
    .split("\n")
    .map((line) => {
      const marker = FENCE.exec(line)?.[1];
      if (fence !== null) {
        if (marker !== undefined && marker[0] === fence[0] && marker.length >= fence.length) fence = null;
        return line;
      }
      if (marker) {
        fence = marker;
        return line;
      }
      return forGithubLine(line);
    })
    .join("\n");
}

/** A flag as a draft comment: its title, the scenario as a numbered list, what was not checked. */
export function flagComment(flag: Annotation): string {
  const scenario = (flag.flag?.scenario ?? []).map((line, i) => `${i + 1}. ${line}`);
  const notChecked = flag.flag?.not_checked.trim();
  const parts = [[flag.text?.trim() ?? "", ...scenario].join("\n")];
  if (notChecked) parts.push(`Not checked: ${notChecked}`);
  return suggested(parts.join("\n\n"));
}

/** An Ask answer as a draft comment, as written (line breaks and lists kept). */
export const answerComment = (answer: string): string => suggested(answer);

/** `text` after an existing draft and a blank line; an empty draft is replaced, one ending with `text` kept. */
export function appendDraft(draft: string, text: string): string {
  const kept = draft.trimEnd();
  const added = text.trimEnd();
  if (!added || kept === added || kept.endsWith(`\n\n${added}`)) return draft;
  return kept ? `${kept}\n\n${text}` : text;
}
