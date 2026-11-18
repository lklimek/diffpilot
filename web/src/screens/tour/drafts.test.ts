import { act, renderHook } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { useCommentDrafts } from "./drafts";

afterEach(() => {
  window.sessionStorage.clear();
  vi.restoreAllMocks();
});

it("keeps drafts per analysis in sessionStorage; a new page reads them back; empty removes", () => {
  const first = renderHook(() => useCommentDrafts("a1"));
  act(() => first.result.current[1]("k96", "unsent"));
  expect(first.result.current[0]).toEqual({ k96: "unsent" });
  first.unmount();
  expect(renderHook(() => useCommentDrafts("a1")).result.current[0]).toEqual({ k96: "unsent" });
  expect(renderHook(() => useCommentDrafts("a2")).result.current[0]).toEqual({});
  const again = renderHook(() => useCommentDrafts("a1"));
  act(() => again.result.current[1]("k96", ""));
  expect(renderHook(() => useCommentDrafts("a1")).result.current[0]).toEqual({});
});

it("without storage (or with damaged data) the drafts still live in memory", () => {
  window.sessionStorage.setItem("smart-review.comment-drafts.a1", "not json");
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
    throw new Error("quota");
  });
  const { result } = renderHook(() => useCommentDrafts("a1"));
  expect(result.current[0]).toEqual({});
  act(() => result.current[1]("k", "text"));
  expect(result.current[0]).toEqual({ k: "text" });
});
