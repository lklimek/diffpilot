import { useEffect, useState, type CSSProperties } from "react";
import type { Highlight, Token } from "./highlight";

interface Result {
  highlight: Highlight;
  path: string;
  lines: readonly string[];
  tokens: Token[][] | null;
}

/**
 * Tokens for `lines` once `highlight` resolves; null meanwhile, on failure and once highlight, path or lines
 * change (plain text). A superseded call is aborted.
 */
export function useTokens(
  highlight: Highlight | undefined,
  path: string,
  lines: readonly string[],
): Token[][] | null {
  const [result, setResult] = useState<Result | null>(null);
  useEffect(() => {
    if (!highlight) return;
    const controller = new AbortController();
    const settle = (tokens: Token[][] | null) => {
      if (!controller.signal.aborted) setResult({ highlight, path, lines, tokens });
    };
    highlight(path, lines, controller.signal).then(settle, () => settle(null));
    return () => controller.abort();
  }, [highlight, path, lines]);
  const current =
    result !== null && result.highlight === highlight && result.path === path && result.lines === lines;
  return current ? result.tokens : null;
}

// Tokens hold escaped text only (T12); they are used only when they reproduce the line exactly.
export function CodeText({ text, tokens }: { text: string; tokens: Token[] | undefined }) {
  if (!tokens || tokens.map((t) => t.content).join("") !== text) return text;
  return tokens.map((t, i) => (
    <span
      key={i}
      data-token
      className="text-(--tok-light) dark:text-(--tok-dark)"
      style={{ "--tok-light": t.light, "--tok-dark": t.dark } as CSSProperties}
    >
      {t.content}
    </span>
  ));
}
