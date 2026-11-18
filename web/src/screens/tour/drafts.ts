// Unsent comment texts of one user and analysis: kept in memory and in sessionStorage, so no navigation loses
// one. Outside React state: typing re-renders only the composer.
import { useEffect, useMemo } from "react";
import type { AnalysisId } from "../../api/generated/types";
import type { CommentDrafts } from "../../components/comment/CommentComposer";

const PREFIX = "smart-review.comment-drafts.";
const storageKey = (login: string, id: AnalysisId) => `${PREFIX}${login}.${id}`;

/** Delay of the storage write after the last change. */
export const DRAFT_WRITE_MS = 300;

// Bumped by `clearCommentDrafts`: stores from before it no longer write.
let generation = 0;

function read(key: string): Record<string, string> {
  try {
    const raw = JSON.parse(window.sessionStorage.getItem(key) ?? "{}") as unknown;
    if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return {};
    return Object.fromEntries(
      Object.entries(raw).filter((e): e is [string, string] => typeof e[1] === "string" && e[1] !== ""),
    );
  } catch {
    return {};
  }
}

/** Removes every user's comment drafts from this tab (signed out, or the session ended). */
export function clearCommentDrafts(): void {
  generation += 1;
  try {
    const keys = Array.from({ length: window.sessionStorage.length }, (_, i) => window.sessionStorage.key(i));
    for (const key of keys) if (key?.startsWith(PREFIX)) window.sessionStorage.removeItem(key);
  } catch {
    // Storage unavailable: nothing was kept there.
  }
}

interface DraftStore extends CommentDrafts {
  /** Writes a pending change now. */
  flush: () => void;
}

function createStore(key: string | null): DraftStore {
  const born = generation;
  const drafts = key === null ? {} : read(key);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const flush = () => {
    if (timer === undefined) return;
    clearTimeout(timer);
    timer = undefined;
    if (key === null || born !== generation) return;
    try {
      window.sessionStorage.setItem(key, JSON.stringify(drafts));
    } catch {
      // Storage unavailable or full: the drafts stay in memory for this page.
    }
  };
  return {
    get: (target) => drafts[target] ?? "",
    set: (target, text) => {
      if ((drafts[target] ?? "") === text) return;
      if (text === "") delete drafts[target];
      else drafts[target] = text;
      clearTimeout(timer);
      timer = setTimeout(flush, DRAFT_WRITE_MS);
    },
    flush,
  };
}

/** Comment drafts of `login` on analysis `id`; without a login they live in memory only. */
export function useCommentDrafts(id: AnalysisId, login: string | null): CommentDrafts {
  const store = useMemo(() => createStore(login === null ? null : storageKey(login, id)), [id, login]);
  useEffect(() => {
    window.addEventListener("pagehide", store.flush);
    return () => {
      window.removeEventListener("pagehide", store.flush);
      store.flush();
    };
  }, [store]);
  return store;
}
