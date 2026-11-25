import { Fragment, useContext, type ReactNode } from "react";
import { inApp } from "../inApp";
import { GithubMark } from "../github/GithubMark";
import { PathLinks } from "./pathLinks";

// An unfinished fence consumes the rest as literal text; single backticks never match inside a longer run.
const TOKENS = /^```[^\n`]*\n[\s\S]*?(?:^```(?=\n|$)|(?![\s\S]))|(?<!`)`[^`\n]+`(?!`)/gm;

// Keep URLs and invalid line suffixes as whole tokens so their component paths cannot become links.
const PATH_TOKEN = /[\p{L}\p{N}_.~/%+@\\:#?=&-]+/gu;

/** Backtick spans, fenced code and analysis file mentions; all other text stays literal. */
export function CodeText({ text, linkPaths = true }: { text: string; linkPaths?: boolean }) {
  const links = useContext(PathLinks);
  const withLinks = (value: string, inline: boolean): ReactNode => {
    if (!links || !linkPaths || !value) return inline ? <code>{value}</code> : value;

    const link = (token: string, key: number) => {
      const target = links.resolve(token);
      return (
        target && (
          <a
            key={key}
            href={target.href}
            title={target.tip}
            className="font-mono underline"
            target={target.github ? "_blank" : undefined}
            rel={target.github ? "noopener noreferrer" : undefined}
            onClick={target.github ? undefined : (event) => inApp(event, () => links.onNavigate(target.href))}
          >
            {token}
            {target.github && <GithubMark />}
          </a>
        )
      );
    };
    if (inline) {
      const whole = link(value, 0);
      if (whole) return <code>{whole}</code>;
    }
    const result: ReactNode[] = [];
    let from = 0;
    for (const match of value.matchAll(PATH_TOKEN)) {
      const token = match[0].replace(/[.?:]+$/, "");
      const anchor = link(token, match.index);
      if (!anchor) continue;
      result.push(value.slice(from, match.index), anchor);
      from = match.index + token.length;
    }
    result.push(value.slice(from));
    return inline ? <code>{result}</code> : result;
  };
  const parts: ReactNode[] = [];
  let from = 0;
  for (const match of text.matchAll(TOKENS)) {
    parts.push(withLinks(text.slice(from, match.index), false));
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
      parts.push(withLinks(token.slice(1, -1), true));
    }
    from = match.index + token.length;
  }
  parts.push(withLinks(text.slice(from), false));
  return (
    <>
      {parts.map((part, i) => (
        <Fragment key={i}>{part}</Fragment>
      ))}
    </>
  );
}
