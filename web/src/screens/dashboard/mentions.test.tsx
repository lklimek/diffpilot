import { act, renderHook } from "@testing-library/react";
import { expect, it } from "vitest";
import { MENTION_BOTS_KEY, MENTION_EXPANDED_KEY, useMentionBots, useMentionsExpanded } from "./mentions";

it("defaults to without bots and remembers the choice across visits", () => {
  localStorage.removeItem(MENTION_BOTS_KEY);
  const first = renderHook(useMentionBots);
  expect(first.result.current[0]).toBe(false);
  act(() => first.result.current[1](true));
  first.unmount();
  const second = renderHook(useMentionBots);
  expect(second.result.current[0]).toBe(true);
  act(() => second.result.current[1](false));
  expect(localStorage.getItem(MENTION_BOTS_KEY)).toBe("false");
});

it("defaults to collapsed and remembers expansion across visits", () => {
  localStorage.removeItem(MENTION_EXPANDED_KEY);
  const first = renderHook(useMentionsExpanded);
  expect(first.result.current[0]).toBe(false);
  act(() => first.result.current[1](true));
  first.unmount();
  const second = renderHook(useMentionsExpanded);
  expect(second.result.current[0]).toBe(true);
});
