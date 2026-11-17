import { Fragment, type ReactNode } from "react";

// An unfinished fence consumes the rest as literal text; single backticks never match inside a longer run.
const TOKENS = /^```[^\n`]*\n[\s\S]*?(?:^```(?=\n|$)|(?![\s\S]))|(?<!`)`[^`\n]+`(?!`)/gm;

/** Only backtick spans and fenced code; React escapes everything, with no HTML or link interpretation. */
export function CodeText({ text }: { text: string }) {
  const parts: ReactNode[] = [];
  let from = 0;
  for (const match of text.matchAll(TOKENS)) {
    parts.push(text.slice(from, match.index));
    const token = match[0];
    if (token.startsWith("```")) {
      parts.push(
        token.endsWith("\n```") ? (
          <pre className="my-1 max-w-full overflow-auto whitespace-pre">
            <code>{token.slice(token.indexOf("\n") + 1, -3)}</code>
          </pre>
        ) : (
          token
        ),
      );
    } else {
      parts.push(<code>{token.slice(1, -1)}</code>);
    }
    from = match.index + token.length;
  }
  parts.push(text.slice(from));
  return (
    <>
      {parts.map((part, i) => (
        <Fragment key={i}>{part}</Fragment>
      ))}
    </>
  );
}
