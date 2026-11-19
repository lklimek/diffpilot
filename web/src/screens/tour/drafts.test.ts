import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { CreateCommentRequest } from "../../api/generated/types";
import { clearCommentDrafts, DRAFT_WRITE_MS, useCommentDrafts, useUnresolvedPosts } from "./drafts";

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  window.sessionStorage.clear();
  vi.restoreAllMocks();
});

const stored = () => JSON.stringify({ ...window.sessionStorage });

it("keeps drafts per user and analysis in sessionStorage; a new page reads them back; empty removes", () => {
  const first = renderHook(() => useCommentDrafts("a1", "alice"));
  act(() => first.result.current.set("k96", "unsent"));
  expect(first.result.current.get("k96")).toBe("unsent");
  first.unmount();
  expect(renderHook(() => useCommentDrafts("a1", "alice")).result.current.get("k96")).toBe("unsent");
  expect(renderHook(() => useCommentDrafts("a2", "alice")).result.current.get("k96")).toBe("");
  expect(renderHook(() => useCommentDrafts("a1", "bob")).result.current.get("k96")).toBe("");
  const again = renderHook(() => useCommentDrafts("a1", "alice"));
  act(() => again.result.current.set("k96", ""));
  again.unmount();
  expect(renderHook(() => useCommentDrafts("a1", "alice")).result.current.get("k96")).toBe("");
});

it("writes the storage once after typing pauses, not on every key press", () => {
  const write = vi.spyOn(Storage.prototype, "setItem");
  const { result } = renderHook(() => useCommentDrafts("a1", "alice"));
  for (const text of ["I", "Is", "Is i", "Is it"]) act(() => result.current.set("k", text));
  expect(write).not.toHaveBeenCalled();
  act(() => vi.advanceTimersByTime(DRAFT_WRITE_MS));
  expect(write).toHaveBeenCalledTimes(1);
  expect(stored()).toContain("Is it");
});

it("writes a pending change when the page is left", () => {
  const { result } = renderHook(() => useCommentDrafts("a1", "alice"));
  act(() => result.current.set("k", "half"));
  window.dispatchEvent(new Event("pagehide"));
  expect(stored()).toContain("half");
});

it("without a signed-in user keeps drafts in memory only", () => {
  window.sessionStorage.setItem("diffpilot.comment-drafts.alice.a1", JSON.stringify({ k: "alice's" }));
  const { result, unmount } = renderHook(() => useCommentDrafts("a1", null));
  expect(result.current.get("k")).toBe("");
  act(() => result.current.set("k", "anonymous"));
  unmount();
  expect(stored()).not.toContain("anonymous");
});

it("sign-out clears that login's drafts and posts only; a store from before does not write them back", () => {
  window.sessionStorage.setItem("diffpilot.comment-drafts.bob.a2", JSON.stringify({ k: "bob's" }));
  window.sessionStorage.setItem("diffpilot.comment-posts.alice.a3", JSON.stringify({}));
  window.sessionStorage.setItem("diffpilot.layout", "kept");
  const { result, unmount } = renderHook(() => useCommentDrafts("a1", "alice"));
  act(() => result.current.set("k", "alice's"));
  clearCommentDrafts("alice");
  unmount();
  expect(stored()).not.toContain("alice");
  expect(stored()).toContain("bob's");
  expect(window.sessionStorage.getItem("diffpilot.layout")).toBe("kept");
});

const request: CreateCommentRequest = {
  idempotency_key: "0b0e7d5c-5c2a-4c55-9a0e-2a7b8f7d1c11",
  target: { path: "src/a.rs", side: "head", start_line: null, line: 5 },
  body: "Is it bounded?",
};

it("keeps unconfirmed posts per user and analysis; one still in flight comes back as not confirmed", () => {
  const first = renderHook(() => useUnresolvedPosts("a1", "alice"));
  act(() => first.result.current[1]((all) => ({ ...all, k: { request, error: null } })));
  first.unmount();
  const [posts] = renderHook(() => useUnresolvedPosts("a1", "alice")).result.current;
  expect(posts.k?.request).toEqual(request);
  expect(posts.k?.error).toMatch(/left before GitHub answered/);
  expect(renderHook(() => useUnresolvedPosts("a1", "bob")).result.current[0]).toEqual({});
  const again = renderHook(() => useUnresolvedPosts("a1", "alice"));
  act(() => again.result.current[1](() => ({})));
  again.unmount();
  expect(renderHook(() => useUnresolvedPosts("a1", "alice")).result.current[0]).toEqual({});
});

it("ignores damaged stored posts", () => {
  window.sessionStorage.setItem(
    "diffpilot.comment-posts.alice.a1",
    JSON.stringify({ a: { request: { body: 1 } }, b: "x", c: { request, error: "GitHub did not answer." } }),
  );
  const [posts] = renderHook(() => useUnresolvedPosts("a1", "alice")).result.current;
  expect(Object.keys(posts)).toEqual(["c"]);
});

it("without storage (or with damaged data) the drafts still live in memory", () => {
  window.sessionStorage.setItem("diffpilot.comment-drafts.alice.a1", "not json");
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
    throw new Error("quota");
  });
  const { result } = renderHook(() => useCommentDrafts("a1", "alice"));
  expect(result.current.get("k")).toBe("");
  act(() => result.current.set("k", "text"));
  act(() => vi.advanceTimersByTime(DRAFT_WRITE_MS));
  expect(result.current.get("k")).toBe("text");
});
