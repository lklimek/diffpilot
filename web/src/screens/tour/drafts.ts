// Unsent comment texts and unconfirmed posts of one user and analysis: kept in memory and in sessionStorage, so
// neither navigation nor a reload loses one. Drafts live outside React state: typing re-renders only the composer.
import { useEffect, useMemo, useState } from "react";
import type { AnalysisId, CreateCommentRequest } from "../../api/generated/types";
import type {
  CommentDrafts,
  UnresolvedPost,
  UnresolvedPosts,
} from "../../components/comment/CommentComposer";

const PREFIX = "smart-review.comment-drafts.";
const POSTS_PREFIX = "smart-review.comment-posts.";
const storageKey = (login: string, id: AnalysisId) => `${PREFIX}${login}.${id}`;
const postsKey = (login: string, id: AnalysisId) => `${POSTS_PREFIX}${login}.${id}`;

/** Error of a post whose request was still running when the page was left: its outcome is unknown. */
const LEFT_IN_FLIGHT = "the page was left before GitHub answered.";

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

/**
 * Removes `login`'s comment drafts and unconfirmed posts from this tab: for an explicit sign-out of that user.
 * An ended session keeps them (the key has the login), so the same user gets them back after signing in.
 */
export function clearCommentDrafts(login: string): void {
  generation += 1;
  const own = [storageKey(login, ""), postsKey(login, "")];
  try {
    const keys = Array.from({ length: window.sessionStorage.length }, (_, i) => window.sessionStorage.key(i));
    for (const key of keys) {
      if (key !== null && own.some((prefix) => key.startsWith(prefix))) window.sessionStorage.removeItem(key);
    }
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

const isRequest = (value: unknown): value is CreateCommentRequest => {
  const r = value as Partial<CreateCommentRequest> | null;
  return (
    typeof r === "object" &&
    r !== null &&
    typeof r.idempotency_key === "string" &&
    typeof r.body === "string" &&
    typeof r.target === "object" &&
    r.target !== null
  );
};

function readPosts(key: string | null): UnresolvedPosts {
  if (key === null) return {};
  try {
    const raw = JSON.parse(window.sessionStorage.getItem(key) ?? "{}") as unknown;
    if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return {};
    const posts: Record<string, UnresolvedPost> = {};
    for (const [target, value] of Object.entries(raw)) {
      const post = value as Partial<UnresolvedPost> | null;
      if (typeof post !== "object" || post === null || !isRequest(post.request)) continue;
      posts[target] = {
        request: post.request,
        error: typeof post.error === "string" ? post.error : LEFT_IN_FLIGHT,
      };
    }
    return posts;
  } catch {
    return {};
  }
}

/**
 * Posts of `login` on analysis `id` whose outcome is not known yet, kept in this tab: after a reload they come
 * back locked (Retry with the same idempotency key, or Discard). Without a login they live in memory only.
 */
export function useUnresolvedPosts(
  id: AnalysisId,
  login: string | null,
): [UnresolvedPosts, (update: (posts: UnresolvedPosts) => UnresolvedPosts) => void] {
  const key = login === null ? null : postsKey(login, id);
  const [state, setState] = useState(() => ({ key, posts: readPosts(key) }));
  if (state.key !== key) setState({ key, posts: readPosts(key) });
  useEffect(() => {
    if (state.key === null) return;
    try {
      if (Object.keys(state.posts).length === 0) window.sessionStorage.removeItem(state.key);
      else window.sessionStorage.setItem(state.key, JSON.stringify(state.posts));
    } catch {
      // Storage unavailable or full: the posts stay in memory for this page.
    }
  }, [state]);
  const update = (change: (posts: UnresolvedPosts) => UnresolvedPosts) =>
    setState((current) => ({ key: current.key, posts: change(current.posts) }));
  return [state.posts, update];
}
