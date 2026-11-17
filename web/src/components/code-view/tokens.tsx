import { useEffect, useState, type CSSProperties } from "react";
import type { Highlight, Token } from "./highlight";

/** Token lists per fragment; null for a fragment left plain. */
export type FragmentTokens = (Token[][] | null)[];

interface Result {
  highlight: Highlight;
  path: string;
  fragments: readonly (readonly string[])[];
  tokens: FragmentTokens | null;
}

/**
 * Tokens per fragment from ONE `highlight` call for the whole view (one budget); null meanwhile, on failure and
 * once highlight, path or fragments change (plain text). A superseded call is aborted.
 */
export function useTokens(
  highlight: Highlight | undefined,
  path: string,
  fragments: readonly (readonly string[])[],
): FragmentTokens | null {
  const [result, setResult] = useState<Result | null>(null);
  useEffect(() => {
    if (!highlight) return;
    const controller = new AbortController();
    const settle = (tokens: FragmentTokens | null) => {
      if (!controller.signal.aborted) setResult({ highlight, path, fragments, tokens });
    };
    highlight(path, fragments, controller.signal).then(settle, () => settle(null));
    return () => controller.abort();
  }, [highlight, path, fragments]);
  const current =
    result !== null &&
    result.highlight === highlight &&
    result.path === path &&
    result.fragments === fragments;
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
