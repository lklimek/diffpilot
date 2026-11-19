import { beforeEach, describe, expect, it } from "vitest";
import { adoptFormerStorageKeys } from "./storageKeys";

describe("adoptFormerStorageKeys", () => {
  beforeEach(() => window.localStorage.clear());

  it("moves values stored under the former name, for both separators", () => {
    window.localStorage.setItem("smart-review.tour.layout", "L");
    window.localStorage.setItem("smart-review:settings:repo:7", "R");
    adoptFormerStorageKeys(() => window.localStorage);
    expect(window.localStorage.getItem("diffpilot.tour.layout")).toBe("L");
    expect(window.localStorage.getItem("diffpilot:settings:repo:7")).toBe("R");
    expect(Object.keys(window.localStorage).filter((key) => key.startsWith("smart-review"))).toEqual([]);
  });

  it("keeps a value already stored under the current name", () => {
    window.localStorage.setItem("smart-review.nav.collapsed", "old");
    window.localStorage.setItem("diffpilot.nav.collapsed", "new");
    adoptFormerStorageKeys(() => window.localStorage);
    expect(window.localStorage.getItem("diffpilot.nav.collapsed")).toBe("new");
    expect(window.localStorage.getItem("smart-review.nav.collapsed")).toBeNull();
  });

  it("leaves other keys alone and survives storage that throws", () => {
    window.localStorage.setItem("sr.dashboard.kinds", "[]");
    window.localStorage.setItem("smart-reviewer", "x");
    adoptFormerStorageKeys(() => window.localStorage);
    expect(window.localStorage.getItem("sr.dashboard.kinds")).toBe("[]");
    expect(window.localStorage.getItem("smart-reviewer")).toBe("x");
    expect(() =>
      adoptFormerStorageKeys(() => {
        throw new Error("blocked");
      }),
    ).not.toThrow();
  });
});
