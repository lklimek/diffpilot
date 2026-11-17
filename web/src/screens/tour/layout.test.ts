import { afterEach, describe, expect, it } from "vitest";
import {
  DEFAULT_LAYOUT,
  LAYOUT_STORAGE_KEY,
  MIN_MAIN_PX,
  STRIP_PX,
  canHide,
  clampWidth,
  hideColumn,
  loadLayout,
  saveLayout,
  showColumn,
  shownColumns,
  sideWidths,
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

  it("lets a column be hidden only while another one stays visible", () => {
    expect(canHide(DEFAULT_LAYOUT, "main")).toBe(true);
    const onlyMain = hideColumn(hideColumn(DEFAULT_LAYOUT, "nav"), "inspector");
    expect(canHide(onlyMain, "main")).toBe(false);
    expect(canHide(onlyMain, "nav")).toBe(true);
    // Full screen leaves the hidden columns as they were: Hide returns to them.
    expect(canHide(toggleFull(DEFAULT_LAYOUT, "main"), "main")).toBe(true);
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

  it("drops stored widths that are not plausible (hostile or broken storage)", () => {
    for (const bad of [1e9, -5, 0, 120, "400", null, { px: 400 }, Number.NaN, "Infinity"]) {
      window.localStorage.setItem(
        LAYOUT_STORAGE_KEY,
        JSON.stringify({ widths: { nav: bad, inspector: bad } }),
      );
      expect(loadLayout().widths).toEqual({ nav: null, inspector: null });
    }
    window.localStorage.setItem(LAYOUT_STORAGE_KEY, '{"widths":{"nav":1e999}}');
    expect(loadLayout().widths.nav).toBeNull();
  });

  it("widths stored on a wide screen leave the centre usable on a narrow one; the preference stays", () => {
    const wide = { ...DEFAULT_LAYOUT, widths: { nav: 1800, inspector: 620 } };
    const drawn = sideWidths(wide, 1440);
    expect(1440 - 12 - drawn.nav - drawn.inspector).toBeGreaterThanOrEqual(MIN_MAIN_PX);
    expect(drawn.nav).toBeGreaterThan(drawn.inspector);
    expect(wide.widths).toEqual({ nav: 1800, inspector: 620 });
    expect(sideWidths(wide, 3440)).toEqual({ nav: 1800, inspector: 620 });
  });

  it("restoring a hidden column after the other grew still leaves the centre its room", () => {
    const grown = { ...hideColumn(DEFAULT_LAYOUT, "inspector"), widths: { nav: 1000, inspector: null } };
    expect(sideWidths(grown, 1440)).toEqual({ nav: 1000, inspector: STRIP_PX });
    const restored = sideWidths(showColumn(grown, "inspector"), 1440);
    expect(1440 - 12 - restored.nav - restored.inspector).toBeGreaterThanOrEqual(MIN_MAIN_PX);
  });

  it("uses the default widths when nothing is stored", () => {
    expect(sideWidths(DEFAULT_LAYOUT, 1440)).toEqual({ nav: 270, inspector: 389 });
    expect(sideWidths(DEFAULT_LAYOUT, 3440)).toEqual({ nav: 400, inspector: 620 });
  });
});
