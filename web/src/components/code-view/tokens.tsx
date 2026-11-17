import { useEffect, useState, type CSSProperties } from "react";
import type { Highlight, Token } from "./highlight";

/** Tokens for `lines` once `highlight` resolves; null meanwhile and on failure (plain text). */
export function useTokens(
  highlight: Highlight | undefined,
  path: string,
  lines: readonly string[],
): Token[][] | null {
  const [result, setResult] = useState<{ lines: readonly string[]; tokens: Token[][] } | null>(null);
  useEffect(() => {
    if (!highlight) return;
    let live = true;
    highlight(path, lines).then(
      (tokens) => {
        if (live && tokens) setResult({ lines, tokens });
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, [highlight, path, lines]);
  return result?.lines === lines ? result.tokens : null;
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
