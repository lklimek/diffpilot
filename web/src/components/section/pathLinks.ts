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
    const file = exact.has(normalized) ? normalized : exact.has(path) ? path : suffixes.get(normalized);
    if (file) return { path: file, line, end };
  };
}
