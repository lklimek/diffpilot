import { describe, expect, it } from "vitest";
import { chatGeometry } from "./chatGeometry";

const size = { width: null, height: null, full: false };
const ws = { top: 40, right: 1600, bottom: 900, width: 1600 };

describe("chatGeometry (proposal AC2, AC14)", () => {
  it("sits over the Inspector, as wide as it, above its dock, at 75 % of the room above", () => {
    const g = chatGeometry(
      { width: 1600, height: 900 },
      ws,
      { left: 1160, right: 1600, width: 440 },
      740,
      size,
    );
    // Its bottom edge touches the dock's top edge: no strip of Inspector text shows between them.
    expect(g).toMatchObject({ right: 8, bottom: 160, width: 424, narrow: false });
    // Room above the dock: 900 - 160 - 48 = 692.
    expect(g.maxHeight).toBe(692);
    expect(g.height).toBe(Math.round(692 * 0.75));
    expect(
      chatGeometry({ width: 1600, height: 900 }, ws, { left: 1160, right: 1600, width: 440 }, 740, {
        ...size,
        full: true,
      }).height,
    ).toBe(692);
  });

  it("keeps a chosen size within the limits", () => {
    const g = chatGeometry({ width: 1600, height: 900 }, ws, { left: 1160, right: 1600, width: 440 }, 740, {
      width: 5000,
      height: 10,
      full: false,
    });
    expect(g.width).toBe(g.maxWidth);
    expect(g.height).toBe(g.minHeight);
  });

  it("with the Inspector hidden sits in the window's bottom-right corner", () => {
    expect(chatGeometry({ width: 1600, height: 900 }, ws, null, null, size)).toMatchObject({
      right: 8,
      bottom: 8,
      width: 460,
    });
  });

  it("is a full-width bottom sheet in a narrow window", () => {
    expect(chatGeometry({ width: 1000, height: 800 }, ws, null, null, size)).toMatchObject({
      narrow: true,
      width: 984,
    });
  });
});
