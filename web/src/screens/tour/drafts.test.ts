import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { clearCommentDrafts, DRAFT_WRITE_MS, useCommentDrafts } from "./drafts";

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
  window.sessionStorage.setItem("smart-review.comment-drafts.alice.a1", JSON.stringify({ k: "alice's" }));
  const { result, unmount } = renderHook(() => useCommentDrafts("a1", null));
  expect(result.current.get("k")).toBe("");
  act(() => result.current.set("k", "anonymous"));
  unmount();
  expect(stored()).not.toContain("anonymous");
});

it("clearing removes every user's drafts; a store from before the clear does not write them back", () => {
  window.sessionStorage.setItem("smart-review.comment-drafts.bob.a2", JSON.stringify({ k: "bob's" }));
  window.sessionStorage.setItem("smart-review.layout", "kept");
  const { result, unmount } = renderHook(() => useCommentDrafts("a1", "alice"));
  act(() => result.current.set("k", "alice's"));
  clearCommentDrafts();
  unmount();
  expect(stored()).not.toContain("comment-drafts");
  expect(window.sessionStorage.getItem("smart-review.layout")).toBe("kept");
});

it("without storage (or with damaged data) the drafts still live in memory", () => {
  window.sessionStorage.setItem("smart-review.comment-drafts.alice.a1", "not json");
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
    throw new Error("quota");
  });
  const { result } = renderHook(() => useCommentDrafts("a1", "alice"));
  expect(result.current.get("k")).toBe("");
  act(() => result.current.set("k", "text"));
  act(() => vi.advanceTimersByTime(DRAFT_WRITE_MS));
  expect(result.current.get("k")).toBe("text");
});
