import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SECTION_STORAGE_KEY, loadSectionState, saveSectionState, useSectionState } from "./sectionState";

beforeEach(() => window.localStorage.clear());
afterEach(() => vi.restoreAllMocks());

describe("section state", () => {
  it("opens every section by default, Flags included, except Assessment", () => {
    const { result } = renderHook(() => useSectionState());
    expect(result.current.isOpen("diff")).toBe(true);
    expect(result.current.isOpen("why")).toBe(true);
    expect(result.current.isOpen("flags")).toBe(true);
    expect(result.current.isOpen("assessment")).toBe(false);
  });

  it("remembers a toggle in localStorage and reads it back", () => {
    const { result } = renderHook(() => useSectionState());
    act(() => result.current.toggle("why"));
    act(() => result.current.toggle("assessment"));
    expect(result.current.isOpen("why")).toBe(false);
    expect(JSON.parse(window.localStorage.getItem(SECTION_STORAGE_KEY)!)).toEqual({
      why: false,
      assessment: true,
    });
    const again = renderHook(() => useSectionState());
    expect(again.result.current.isOpen("why")).toBe(false);
    expect(again.result.current.isOpen("assessment")).toBe(true);
  });

  it("reveals a section for the current scope without storing it; a toggle after it is stored", () => {
    const { result, rerender } = renderHook(({ scope }) => useSectionState(scope), {
      initialProps: { scope: "f1.5" },
    });
    act(() => result.current.reveal("assessment"));
    expect(result.current.isOpen("assessment")).toBe(true);
    expect(window.localStorage.getItem(SECTION_STORAGE_KEY)).toBeNull();
    rerender({ scope: "f1.6" });
    expect(result.current.isOpen("assessment")).toBe(false);
    rerender({ scope: "f1.5" });
    expect(result.current.isOpen("assessment")).toBe(false);
    act(() => result.current.reveal("assessment"));
    act(() => result.current.toggle("assessment"));
    expect(result.current.isOpen("assessment")).toBe(false);
    expect(JSON.parse(window.localStorage.getItem(SECTION_STORAGE_KEY)!)).toEqual({ assessment: false });
  });

  it.each([
    ["not JSON", "{"],
    ["an array", "[true]"],
    ["null", "null"],
  ])("ignores a stored value that is %s", (_, raw) => {
    window.localStorage.setItem(SECTION_STORAGE_KEY, raw);
    expect(loadSectionState()).toEqual({});
  });

  it("drops stored entries that are not booleans", () => {
    window.localStorage.setItem(SECTION_STORAGE_KEY, JSON.stringify({ why: false, diff: "no" }));
    expect(loadSectionState()).toEqual({ why: false });
  });

  it("works in memory when reading and writing storage throw", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new DOMException("denied", "SecurityError");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("full", "QuotaExceededError");
    });
    const { result } = renderHook(() => useSectionState());
    expect(result.current.isOpen("why")).toBe(true);
    act(() => result.current.toggle("why"));
    expect(result.current.isOpen("why")).toBe(false);
  });

  it("works when localStorage itself cannot be accessed", () => {
    const spy = vi.spyOn(window, "localStorage", "get").mockImplementation(() => {
      throw new DOMException("denied", "SecurityError");
    });
    expect(loadSectionState()).toEqual({});
    expect(() => saveSectionState({ why: false })).not.toThrow();
    expect(spy).toHaveBeenCalled();
  });
});
