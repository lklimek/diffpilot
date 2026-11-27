import { createContext } from "react";
import type { RepoRef } from "../../api/generated/types";

export interface FileMention {
  path: string;
  line?: number;
  end?: number;
}

export interface FileMentionLink {
  href: string;
  tip: string;
  github?: boolean;
}

// Keep URLs and invalid line suffixes as whole tokens so their component paths cannot become links.
const PATH_TOKEN = /[\p{L}\p{N}_.~/%+@\\:#?=&-]+/gu;

export function* fileMentionTokens(text: string) {
  for (const match of text.matchAll(PATH_TOKEN)) {
    yield { token: match[0].replace(/[.?:]+$/, ""), index: match.index };
  }
}

/** A cheap preflight for views that otherwise have no Files index. */
export function hasFileMentionSyntax(text: string): boolean {
  if (/(?<!`)`[\p{L}\p{N}_-]+`(?!`)/u.test(text)) return true;
  for (const { token } of fileMentionTokens(text)) {
    if (token.startsWith("/") || token.includes("://")) continue;
    if (/^(?:Makefile|Dockerfile|Containerfile|README|LICENSE|COPYING|NOTICE)$/.test(token)) return true;
    if (/\/|\.[\p{L}\p{N}_]+$|:\d+(?:-\d+)?$|#L\d+$/u.test(token)) return true;
  }
  return false;
}

export const PathLinks = createContext<{
  resolve: (mention: string) => FileMentionLink | undefined;
  onNavigate: (href: string) => void;
} | null>(null);

/** Resolve only exact paths and unambiguous suffixes of files in this analysis. */
export function pathMatcher(paths: readonly string[], repo: RepoRef) {
  const exact = new Set(paths);
  const suffixes = new Map<string, string | null>();
  for (const path of exact) {
    for (let slash = path.indexOf("/"); slash >= 0; slash = path.indexOf("/", slash + 1)) {
      const suffix = path.slice(slash + 1);
      if (suffix.includes("/")) suffixes.set(suffix, suffixes.has(suffix) ? null : path);
    }
  }

  return (mention: string): FileMention | undefined => {
    const parts = /^([^:#\\]+?)(?::(\d+)(?:-(\d+))?|#L(\d+))?$/.exec(mention);
    if (!parts) return;
    const [, raw, start, finish, hash] = parts;
    const path = raw!.replace(/^\.\//, "");
    if (path.startsWith("/") || path.split("/").some((part) => !part || part === "." || part === ".."))
      return;
    const line = start || hash ? Number(start ?? hash) : undefined;
    const end = finish ? Number(finish) : undefined;
    if (line !== undefined && (!Number.isSafeInteger(line) || line < 1)) return;
    if (end !== undefined && (!Number.isSafeInteger(end) || end < line!)) return;

    const prefix = [`${repo.owner}/${repo.name}/`, `${repo.name}/`].find((root) => path.startsWith(root));
    const normalized = prefix ? path.slice(prefix.length) : path;
    const file = exact.has(path) ? path : exact.has(normalized) ? normalized : suffixes.get(normalized);
    if (file) return { path: file, line, end };
  };
}
