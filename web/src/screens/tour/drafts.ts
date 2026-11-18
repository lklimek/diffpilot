// Unsent comment texts of one analysis: kept in memory and in sessionStorage, so no navigation loses one.
import { useState } from "react";
import type { AnalysisId } from "../../api/generated/types";
import type { CommentDrafts } from "../../components/comment/CommentComposer";

const storageKey = (id: AnalysisId) => `smart-review.comment-drafts.${id}`;

function read(id: AnalysisId): CommentDrafts {
  try {
    const raw = JSON.parse(window.sessionStorage.getItem(storageKey(id)) ?? "{}") as unknown;
    if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return {};
    return Object.fromEntries(
      Object.entries(raw).filter((e): e is [string, string] => typeof e[1] === "string" && e[1] !== ""),
    );
  } catch {
    return {};
  }
}

/** The analysis's comment drafts and their setter; an empty text removes a draft. */
export function useCommentDrafts(id: AnalysisId): [CommentDrafts, (key: string, text: string) => void] {
  const [drafts, setDrafts] = useState(() => read(id));
  const setDraft = (key: string, text: string) =>
    setDrafts((all) => {
      if ((all[key] ?? "") === text) return all;
      const next = { ...all };
      if (text === "") delete next[key];
      else next[key] = text;
      try {
        window.sessionStorage.setItem(storageKey(id), JSON.stringify(next));
      } catch {
        // Storage unavailable or full: the drafts stay in memory for this page.
      }
      return next;
    });
  return [drafts, setDraft];
}
