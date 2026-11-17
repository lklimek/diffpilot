import { afterEach, describe, expect, it, vi } from "vitest";
import { TEXT_FONT, canvasMeasure, estimateWidth } from "./measure";

describe("estimateWidth", () => {
  it("is wider for capitals and wide letters than for lower case, and grows with the font", () => {
    expect(estimateWidth("MAX_CONCURRENT_REQUESTS", "node")).toBeGreaterThan(
      estimateWidth("max_concurrent_requests", "node"),
    );
    expect(estimateWidth("WWWW", "node")).toBeGreaterThan(estimateWidth("iiii", "node"));
    expect(estimateWidth("検証", "node")).toBeGreaterThanOrEqual(24);
    expect(estimateWidth("abc", "number")).toBeGreaterThan(estimateWidth("abc", "node"));
  });

  // DejaVu Sans 12 px, the system-ui face on this host, measures MAX_CONCURRENT_REQUESTS at about 188 px.
  it("does not under-measure a capital identifier in a wide system face", () => {
    expect(estimateWidth("MAX_CONCURRENT_REQUESTS", "node")).toBeGreaterThanOrEqual(188);
  });
});

describe("canvasMeasure", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("is unavailable without OffscreenCanvas", () => {
    vi.stubGlobal("OffscreenCanvas", undefined);
    expect(canvasMeasure("system-ui")).toBeNull();
  });

  it("measures with the page font per text role and caches the result", () => {
    const fonts: string[] = [];
    const context = {
      font: "",
      measureText(text: string) {
        fonts.push(this.font);
        return { width: text.length * 10.2 };
      },
    };
    vi.stubGlobal(
      "OffscreenCanvas",
      class {
        getContext() {
          return context;
        }
      },
    );
    const measure = canvasMeasure('"DejaVu Sans", sans-serif')!;
    expect(measure("abc", "edge")).toBe(31);
    expect(measure("abc", "edge")).toBe(31);
    expect(measure("abc", "number")).toBe(31);
    expect(fonts).toEqual([
      `${TEXT_FONT.edge} "DejaVu Sans", sans-serif`,
      `${TEXT_FONT.number} "DejaVu Sans", sans-serif`,
    ]);
  });
});
