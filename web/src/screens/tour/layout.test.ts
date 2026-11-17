import { afterEach, describe, expect, it } from "vitest";
import {
  DEFAULT_LAYOUT,
  LAYOUT_STORAGE_KEY,
  clampWidth,
  hideColumn,
  loadLayout,
  saveLayout,
  showColumn,
  shownColumns,
  toggleFull,
} from "./layout";

afterEach(() => window.localStorage.clear());

describe("Tour layout (brief §4.3 columns)", () => {
  it("starts with every column shown and the diagram hidden", () => {
    expect(loadLayout()).toEqual(DEFAULT_LAYOUT);
    expect(shownColumns(DEFAULT_LAYOUT)).toEqual({ nav: true, main: true, inspector: true });
    expect(DEFAULT_LAYOUT.diagram).toBe(false);
  });

  it("remembers hidden columns, full screen, widths and the diagram", () => {
    const layout = { ...toggleFull(hideColumn(DEFAULT_LAYOUT, "nav"), "main"), diagram: true };
    saveLayout({ ...layout, widths: { nav: 320, inspector: 480 } });
    expect(loadLayout()).toEqual({ ...layout, widths: { nav: 320, inspector: 480 } });
  });

  it("ignores a stored value of the wrong shape, and never restores all columns hidden", () => {
    window.localStorage.setItem(LAYOUT_STORAGE_KEY, "not json");
    expect(loadLayout()).toEqual(DEFAULT_LAYOUT);
    window.localStorage.setItem(
      LAYOUT_STORAGE_KEY,
      JSON.stringify({ hidden: { nav: true, main: true, inspector: true }, full: "x", widths: { nav: -1 } }),
    );
    expect(loadLayout()).toEqual(DEFAULT_LAYOUT);
  });

  it("hides a column to a strip and shows it again", () => {
    const hidden = hideColumn(DEFAULT_LAYOUT, "inspector");
    expect(shownColumns(hidden)).toEqual({ nav: true, main: true, inspector: false });
    expect(shownColumns(showColumn(hidden, "inspector")).inspector).toBe(true);
  });

  it("never hides the last shown column", () => {
    const one = hideColumn(hideColumn(DEFAULT_LAYOUT, "nav"), "inspector");
    expect(hideColumn(one, "main")).toBe(one);
  });

  it("full screen shows one column; again, or showing another column, leaves it", () => {
    const full = toggleFull(hideColumn(DEFAULT_LAYOUT, "inspector"), "main");
    expect(shownColumns(full)).toEqual({ nav: false, main: true, inspector: false });
    expect(shownColumns(toggleFull(full, "main"))).toEqual({ nav: true, main: true, inspector: false });
    expect(shownColumns(showColumn(full, "nav"))).toEqual({ nav: true, main: true, inspector: false });
  });

  it("hiding a column leaves full screen", () => {
    const full = toggleFull(DEFAULT_LAYOUT, "main");
    expect(hideColumn(full, "nav")).toMatchObject({ full: null, hidden: { nav: true } });
  });

  it("keeps a dragged width within the side minimum and the room of the centre", () => {
    expect(clampWidth("nav", 50, 1440, 400)).toBe(200);
    expect(clampWidth("inspector", 100, 1440, 300)).toBe(300);
    expect(clampWidth("nav", 1200, 1440, 400)).toBe(1440 - 400 - 12 - 320);
  });
});
