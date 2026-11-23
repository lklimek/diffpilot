type Part = { text: string; code: boolean };
type Details = { start: number; summary: string[] | null; prompt: boolean };

const PROMPT = /\bprompt (?:to fix|for ai agents)\b/i;
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
  const markerStarts: number[] = [];
  const omitted: { start: number; end: number }[] = [];
  const current = () => details.at(-1);
  const append = (text: string, code = false) => {
    if (!text || hidden.length) return;
    current()?.summary?.push(text);
    parts.push({ text, code });
  };
  const lineBreak = () => {
    const last = parts.at(-1);
    if (last && !last.text.endsWith("\n")) append("\n");
  };
  const finishSummary = () => {
    const block = current();
    if (block?.summary) {
      block.prompt ||= PROMPT.test(plainMarkdown(block.summary.join("")));
      block.summary = null;
    }
  };
  // Quoted attributes, code and comments are consumed before any tag structure.
  const tokens =
    /^ {0,3}(`{3,}|~{3,})[^\n]*(?:\n|$)|`+|~{3,}|<!--[\s\S]*?(?:-->|$)|<[a-zA-Z][\w+.-]*:[^\s<>]+>|<\/?[a-zA-Z][\w:-]*(?=[\s/>]|$)(?:"[^"]*(?:"|$)|'[^']*(?:'|$)|[^'">])*(?:>|$)/gm;
  let from = 0;
  for (let token = tokens.exec(body); token; token = tokens.exec(body)) {
    append(body.slice(from, token.index));
    const raw = token[0];
    from = tokens.lastIndex;
    if (token[1]) {
      const fence = token[1];
      const end = /^ {0,3}(`+|~+)[ \t\r]*(?=\n|$)/gm;
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
      const marker = /^<!--\s*ai-agent-prompt\s*:\s*(start|end)\s*-->$/i.exec(raw);
      if (marker?.[1]?.toLowerCase() === "start") markerStarts.push(parts.length);
      else if (marker) {
        const start = markerStarts.pop();
        if (start !== undefined) omitted.push({ start, end: parts.length });
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
      if (name === "a") {
        if (closing) {
          const url = anchors.pop();
          if (url) append(` (${url})`);
        } else {
          anchors.push(linkUrl(raw));
        }
      } else if (name === "details") {
        if (closing) {
          finishSummary();
          const block = details.pop();
          lineBreak();
          if (block?.prompt) omitted.push({ start: block.start, end: parts.length });
        } else {
          lineBreak();
          details.push({ start: parts.length, summary: null, prompt: false });
        }
      } else if (name === "summary") {
        if (closing) {
          finishSummary();
        } else {
          const block = current();
          if (block) block.summary = [];
        }
        lineBreak();
      } else if (BLOCK_TAG.test(name)) lineBreak();
    }
  }
  append(body.slice(from));
  for (const url of anchors.reverse()) if (url) append(` (${url})`);
  // Only completed structures hide text; overlapping ranges are combined in one linear pass.
  const boundaries = new Int32Array(parts.length + 1);
  for (const { start, end } of omitted) {
    boundaries[start]!++;
    boundaries[end]!--;
  }
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
  let omittedDepth = 0;
  for (let index = 0; index < parts.length; index++) {
    omittedDepth += boundaries[index]!;
    if (omittedDepth) continue;
    const part = parts[index]!;
    if (part.code) {
      flush();
      output.push(part.text);
    } else prose.push(part.text);
  }
  flush();
  return output.join("").trim();
}
