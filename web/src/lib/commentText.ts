type Part = { text: string; code: boolean };
type Details = { start: number; summary: string[] | null; prompt: boolean };

const PROMPT = /\bprompt (?:to fix (?:the )?issue|for ai agents)\b/i;
const BLOCK_TAG =
  /^(?:section|article|header|footer|main|aside|nav|details|summary|p|div|br|hr|dl|dd|dt|li|ul|ol|h[1-6]|blockquote|tr)$/;
const HTML_TAGS = new Set(
  `a abbr address area article aside audio b base bdi bdo blockquote body br button canvas caption
  cite code col colgroup data datalist dd del details dfn dialog div dl dt em embed fieldset figcaption
  figure footer form h1 h2 h3 h4 h5 h6 head header hgroup hr html i iframe img input ins kbd label
  legend li link main map mark menu meta meter nav noscript object ol optgroup option output p picture
  pre progress q rp rt ruby s samp script search section select slot small source span strong style sub
  summary sup svg table tbody td template textarea tfoot th thead time title tr track u ul var video wbr`.split(
    /\s+/,
  ),
);

function plainEmphasis(text: string): string {
  const literals: { start: number; end: number }[] = [];
  for (const token of text.matchAll(/\S+/g)) {
    if (/[\\/=?#]|[a-z\d]:[^*_]/i.test(token[0])) {
      literals.push({ start: token.index, end: token.index + token[0].length });
    }
  }
  let literal = 0;
  return text.replace(/(\*\*|__|\*|_)([^*_\n]*[^*_\s])\1/g, (raw, _delimiter, inner, offset: number) => {
    const end = offset + raw.length;
    while (literals[literal] && literals[literal]!.end <= offset) literal++;
    if (literals[literal] && literals[literal]!.start < end) return raw;
    const before = text[offset - 1];
    const after = text[end];
    return (before === undefined || /[\s([{"']/.test(before)) &&
      (after === undefined || /[\s)\]}"'.,;!?:]/.test(after)) &&
      !(after === "." && /\w/.test(text[end + 1] ?? ""))
      ? inner
      : raw;
  });
}

/** Pair parentheses once, including malformed links, without rescanning any suffix. */
function plainMarkdown(text: string): string {
  const ends = new Map<number, number>();
  const stack: number[] = [];
  for (let i = 0; i < text.length; i++) {
    if (text[i] === "\\") i++;
    else if (text[i] === "\n") stack.length = 0;
    else if (text[i] === "(") stack.push(i);
    else if (text[i] === ")" && stack.length) ends.set(stack.pop()!, i);
  }
  const links = /(!?)\[([^[\]\n]*)\]\(/g;
  const parts: string[] = [];
  let from = 0;
  for (let link = links.exec(text); link; link = links.exec(text)) {
    const end = ends.get(links.lastIndex - 1);
    if (end === undefined) continue;
    parts.push(
      text.slice(from, link.index),
      link[1] ? "" : `${link[2]} (${text.slice(links.lastIndex, end)})`,
    );
    from = links.lastIndex = end + 1;
  }
  parts.push(text.slice(from));
  return plainEmphasis(parts.join(""));
}

function isPromptHeading(text: string): boolean {
  return PROMPT.test(plainMarkdown(text).replace(/\*\*|__/g, ""));
}

/** Index exact backtick runs on each line so unmatched delimiters never rescan the body. */
function codeEnds(body: string): Map<number, number> {
  const ends = new Map<number, number>();
  const previous = new Map<number, number>();
  for (const run of body.matchAll(/`+|\n/g)) {
    if (run[0] === "\n") previous.clear();
    else {
      const start = previous.get(run[0].length);
      if (start !== undefined) ends.set(start, run.index);
      previous.set(run[0].length, run.index);
    }
  }
  return ends;
}

function linkUrl(tag: string): string {
  const attributes = /([^\s"'<>/=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?/g;
  for (const attribute of tag.matchAll(attributes)) {
    if (attribute[1]!.toLowerCase() === "href") return attribute[2] ?? attribute[3] ?? attribute[4] ?? "";
  }
  return "";
}

/** Display-only GitHub text: strip known markup and agent sections; keep code and ambiguous text literal. */
export function commentText(body: string): string {
  const parts: Part[] = [];
  const details: Details[] = [];
  const hidden: string[] = [];
  const anchors: string[] = [];
  const inlineEnds = codeEnds(body);
  let promptDetails = 0;
  let agentPrompt = false;
  let promptHeading: { level: number; depth: number } | null = null;
  let heading: { name: string; level: number; start: number; position: number; text: string[] } | null = null;
  const current = () => details.at(-1);
  const lineEnd = /[ \t]*(?=\n|$)/y;
  const headingBoundary = (index: number) =>
    /(?:^|\n) {0,3}$/.test(body.slice(Math.max(0, index - 4), index));
  const headingLine = (start: number, end: number) => {
    lineEnd.lastIndex = end;
    return headingBoundary(start) && lineEnd.test(body) && !body.slice(start, end).includes("\n");
  };
  const append = (text: string, code = false) => {
    if (!text || hidden.length) return;
    heading?.text.push(code ? "" : text);
    current()?.summary?.push(code ? "" : text);
    if (!agentPrompt && promptHeading === null && !promptDetails) parts.push({ text, code });
  };
  const lineBreak = () => {
    const last = parts.at(-1);
    if (last && !last.text.endsWith("\n")) append("\n");
  };
  const startHeading = (level: number) => {
    if (promptHeading !== null && level <= promptHeading.level) promptHeading = null;
  };
  const markPromptDetails = () => {
    const block = current();
    if (block && !block.prompt && isPromptHeading(block.summary?.join("") ?? "")) {
      block.prompt = true;
      promptDetails++;
      parts.length = Math.min(parts.length, block.start);
    }
  };
  const closeDetails = () => {
    markPromptDetails();
    if (details.length && promptHeading && promptHeading.depth >= details.length) promptHeading = null;
    if (details.pop()?.prompt) promptDetails--;
    lineBreak();
  };
  // Quoted attributes, code and comments are consumed before any heading or tag structure.
  const tokens =
    /^ {0,3}(`{3,}|~{3,})[^\n]*(?:\n|$)|`+|~{3,}|<!--[\s\S]*?(?:-->|$)|<[a-zA-Z][\w+.-]*:[^\s<>]+>|<\/?[a-zA-Z][\w:-]*(?=[\s/>]|$)(?:"[^"]*(?:"|$)|'[^']*(?:'|$)|[^'">])*(?:>|$)|^ {0,3}#{1,6}[ \t]+[^#<`\n]*|\*\*[^*<`\n]+\*\*|__[^_<`\n]+__|^[^<`\n]+\n {0,3}(?:={3,}|-{3,})[ \t]*(?=\n|$)/gm;
  let from = 0;
  for (let token = tokens.exec(body); token; token = tokens.exec(body)) {
    append(body.slice(from, token.index));
    const raw = token[0];
    from = tokens.lastIndex;
    if (token[1]) {
      const fence = token[1];
      const end = /^ {0,3}(`+|~+)[ \t]*(?=\n|$)/gm;
      end.lastIndex = from;
      let closing: RegExpExecArray | null;
      do {
        closing = end.exec(body);
      } while (closing && (closing[1]![0] !== fence[0] || closing[1]!.length < fence.length));
      from = closing ? end.lastIndex : body.length;
      append(body.slice(token.index, from), true);
      tokens.lastIndex = from;
    } else if (raw.startsWith("`") || raw.startsWith("~~~")) {
      const closing = inlineEnds.get(token.index);
      if (closing !== undefined) {
        from = closing + raw.length;
        append(body.slice(token.index, from), true);
        tokens.lastIndex = from;
      } else if (raw.length >= 3) {
        // Flattened excerpts can end before a fence closes; the remaining code stays literal.
        append(body.slice(token.index), true);
        from = tokens.lastIndex = body.length;
      } else append(raw);
    } else if (raw.startsWith("<!--")) {
      if (hidden.length) continue;
      if (/ai-agent-prompt\s*:\s*start/i.test(raw)) agentPrompt = true;
      if (/ai-agent-prompt\s*:\s*end/i.test(raw)) {
        agentPrompt = false;
        promptHeading = null;
      }
    } else if (!raw.startsWith("<")) {
      const hashes = raw.trimStart().match(/^#+/);
      const level = hashes ? hashes[0].length : raw.includes("\n") ? (raw.includes("=") ? 1 : 2) : 7;
      const prompt = isPromptHeading(raw);
      if (
        heading ||
        current()?.summary ||
        hidden.length ||
        agentPrompt ||
        promptDetails ||
        (level === 7 && !headingLine(token.index, from))
      )
        append(raw);
      else {
        startHeading(level);
        if (prompt) promptHeading = { level, depth: details.length };
        else append(raw);
      }
    } else if (/^<[a-zA-Z][\w+.-]*:/.test(raw)) {
      append(raw.slice(1, -1));
    } else {
      const tag = /^<(\/)?([\w:-]+)/.exec(raw)!;
      const closing = !!tag[1];
      const name = tag[2]!.toLowerCase();
      if (!HTML_TAGS.has(name)) {
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
      if (
        !agentPrompt &&
        !promptDetails &&
        !current()?.summary &&
        (/^h[1-6]$/.test(name) ||
          ((name === "strong" || name === "b") && (!heading || heading.name === name)))
      ) {
        if (!closing) {
          const level: number = name.startsWith("h") ? Number(name[1]) : 7;
          if (level < 7) {
            startHeading(level);
            lineBreak();
          }
          if (level < 7 || headingBoundary(token.index)) {
            heading = { name, level, start: parts.length, position: token.index, text: [] };
          }
        } else if (heading?.name === name) {
          const finished = heading;
          heading = null;
          if (finished.level < 7 || headingLine(finished.position, from)) {
            const wasHidden = promptHeading !== null;
            startHeading(finished.level);
            if (isPromptHeading(finished.text.join(""))) {
              parts.length = Math.min(parts.length, finished.start);
              promptHeading = { level: finished.level, depth: details.length };
            } else if (wasHidden && promptHeading === null) append(finished.text.join(""));
          }
          if (finished.level < 7) lineBreak();
        }
      } else if (name === "a") {
        if (closing) {
          const url = anchors.pop();
          if (url) append(` (${url})`);
        } else {
          anchors.push(linkUrl(raw));
        }
      } else if (name === "details") {
        if (closing) closeDetails();
        else {
          lineBreak();
          details.push({ start: parts.length, summary: null, prompt: false });
        }
      } else if (name === "summary") {
        if (closing) {
          markPromptDetails();
          const block = current();
          if (block) block.summary = null;
        } else {
          const block = current();
          if (block) block.summary = [];
        }
        lineBreak();
      } else if (BLOCK_TAG.test(name)) lineBreak();
    }
  }
  append(body.slice(from));
  while (details.length) closeDetails();
  for (const url of anchors.reverse()) if (url) append(` (${url})`);
  // Join prose before normalising it: markup may split a Markdown link or emphasis span.
  const output: string[] = [];
  let prose: string[] = [];
  const flush = () => {
    output.push(
      plainMarkdown(prose.join(""))
        .replace(/[ \t]+\n?/g, (run) => (run.endsWith("\n") ? "\n" : run))
        .replace(/\n(?:[ \t]*\n){2,}/g, "\n\n"),
    );
    prose = [];
  };
  for (const part of parts) {
    if (part.code) {
      flush();
      output.push(part.text);
    } else prose.push(part.text);
  }
  flush();
  return output.join("").trim();
}
