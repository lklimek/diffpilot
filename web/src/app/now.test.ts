import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NOW_TICK_MS, useNow } from "./now";

afterEach(() => vi.useRealTimers());

describe("useNow", () => {
  it("ticks with the real clock every NOW_TICK_MS and stops on unmount", () => {
    vi.useFakeTimers();
    vi.setSystemTime(Date.parse("2026-11-17T10:00:00Z"));
    const { result, unmount } = renderHook(() => useNow());
    const start = result.current;
    expect(start).toBe(Date.now());
    act(() => vi.advanceTimersByTime(NOW_TICK_MS));
    expect(result.current).toBe(start + NOW_TICK_MS);
    act(() => vi.advanceTimersByTime(3 * NOW_TICK_MS));
    expect(result.current).toBe(start + 4 * NOW_TICK_MS);
    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
});
