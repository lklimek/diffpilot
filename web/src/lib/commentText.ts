type Part = { text: string; code: boolean };
type Block = { parts: Part[]; summary: string | null; prompt: boolean };

const PROMPT = /\bprompt (?:to fix (?:the )?issue|for ai agents)\b/i;
const BLOCK_TAG = /^(?:details|summary|p|div|br|hr|dl|dd|dt|li|ul|ol|h[1-6]|blockquote|tr)$/;
const HTML_TAG =
  /^(?:a|abbr|b|bdi|bdo|blockquote|br|caption|code|dd|del|details|div|dl|dt|em|h[1-6]|hr|i|img|input|kbd|li|ol|p|picture|pre|s|script|small|source|span|strong|style|sub|summary|sup|table|tbody|td|th|thead|tr|ul|video)$/;

/** Markdown links become label plus literal destination; images have no display text. */
function plainMarkdown(text: string): string {
  const links = /(!?)\[([^\]\n]*)\]\(/g;
  const parts: string[] = [];
  let from = 0;
  for (let link = links.exec(text); link; link = links.exec(text)) {
    let depth = 1;
    for (let end = links.lastIndex; end < text.length && text[end] !== "\n"; end++) {
      if (text[end] === "\\") {
        end++;
        continue;
      }
      if (text[end] === "(") depth++;
      if (text[end] !== ")" || --depth !== 0) continue;
      parts.push(
        text.slice(from, link.index),
        link[1] ? "" : `${link[2]} (${text.slice(links.lastIndex, end)})`,
      );
      from = links.lastIndex = end + 1;
      break;
    }
  }
  parts.push(text.slice(from));
  return parts.join("").replace(/\*\*|__/g, "");
}

/** Display-only GitHub text: no HTML, images, agent prompts or active links; code stays literal. */
export function commentText(body: string): string {
  const root: Block = { parts: [], summary: null, prompt: false };
  const blocks = [root];
  const hidden: string[] = [];
  let agentPrompt = false;
  let promptHeading: number | null = null;
  const current = () => blocks[blocks.length - 1]!;
  const append = (text: string, code = false) => {
    if (!text || agentPrompt || promptHeading !== null || hidden.length) return;
    const block = current();
    if (block.summary !== null) block.summary += text;
    const last = block.parts.at(-1);
    if (last && !last.code && !code) last.text += text;
    else block.parts.push({ text, code });
  };
  const lineBreak = () => {
    const last = current().parts.at(-1);
    if (last && !last.text.endsWith("\n")) append("\n");
  };
  const closeDetails = () => {
    if (blocks.length === 1) return;
    const block = blocks.pop()!;
    if (!block.prompt && !PROMPT.test(plainMarkdown(block.summary ?? "")))
      for (const part of block.parts) append(part.text, part.code);
    lineBreak();
  };
  // Code is consumed before markup, so HTML-looking text in a span or fence never changes structure.
  const tokens =
    /^ {0,3}(`{3,}|~{3,})[^\n]*(?:\n|$)|`+|<!--[\s\S]*?(?:-->|$)|<\/?[a-zA-Z][\w:-]*(?=[\s/>]|$)(?:"[^"]*(?:"|$)|'[^']*(?:'|$)|[^'">])*(?:>|$)|^ {0,3}#{1,6}[^\n]*/gm;
  let from = 0;
  for (let token = tokens.exec(body); token; token = tokens.exec(body)) {
    append(body.slice(from, token.index));
    const raw = token[0];
    from = tokens.lastIndex;
    if (token[1]) {
      const fence = token[1];
      const end = new RegExp(`^ {0,3}${fence[0]}{${fence.length},}[ \\t]*(?=\\n|$)`, "gm");
      end.lastIndex = from;
      const closing = end.exec(body);
      from = closing ? end.lastIndex : body.length;
      append(body.slice(token.index, from), true);
      tokens.lastIndex = from;
    } else if (raw.startsWith("`")) {
      const end = new RegExp(`(?<!\x60)\x60{${raw.length}}(?!\x60)`, "g");
      end.lastIndex = from;
      const closing = end.exec(body);
      if (closing && !body.slice(from, closing.index).includes("\n")) {
        from = end.lastIndex;
        append(body.slice(token.index, from), true);
        tokens.lastIndex = from;
      } else append(raw);
    } else if (raw.startsWith("<!--")) {
      if (/ai-agent-prompt\s*:\s*start/i.test(raw)) agentPrompt = true;
      if (/ai-agent-prompt\s*:\s*end/i.test(raw)) agentPrompt = false;
    } else if (raw.trimStart().startsWith("#")) {
      const level = raw.trimStart().match(/^#+/)![0].length;
      if (promptHeading !== null && level <= promptHeading) promptHeading = null;
      if (PROMPT.test(raw)) promptHeading = level;
      else append(raw);
    } else {
      const tag = /^<(\/)?([\w:-]+)/.exec(raw)!;
      const closing = !!tag[1];
      const name = tag[2]!.toLowerCase();
      if (!HTML_TAG.test(name)) {
        append(raw);
        continue;
      }
      if (["picture", "script", "style"].includes(name)) {
        if (closing) {
          if (hidden.at(-1) === name) hidden.pop();
        } else if (!raw.endsWith("/>")) hidden.push(name);
        continue;
      }
      if (hidden.length) continue;
      if (name === "details") {
        if (closing) closeDetails();
        else {
          lineBreak();
          blocks.push({ parts: [], summary: null, prompt: false });
        }
      } else if (name === "summary") {
        if (closing) {
          if (PROMPT.test(plainMarkdown(current().summary ?? ""))) current().prompt = true;
          current().summary = null;
        } else current().summary = "";
        lineBreak();
      } else if (BLOCK_TAG.test(name)) lineBreak();
    }
  }
  append(body.slice(from));
  while (blocks.length > 1) closeDetails();
  return root.parts
    .map((part) =>
      part.code
        ? part.text
        : plainMarkdown(part.text)
            .replace(/[ \t]+\n/g, "\n")
            .replace(/\n(?:[ \t]*\n){2,}/g, "\n\n"),
    )
    .join("")
    .trim();
}
