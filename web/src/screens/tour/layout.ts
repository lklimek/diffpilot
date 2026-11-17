// Layout of the review screen (brief §4.3): three columns, each hideable and full screen, two resizable widths, the
// flow diagram pane; remembered in localStorage.

/** A column of the review screen. */
export type Column = "nav" | "main" | "inspector";

/** All columns, left to right. */
export const COLUMNS: readonly Column[] = ["nav", "main", "inspector"];

/** Remembered layout. */
export interface TourLayout {
  /** Columns hidden by the user (shown as strips). */
  hidden: Record<Column, boolean>;
  /** The column shown alone, if any; the others are strips meanwhile. */
  full: Column | null;
  /** Dragged widths in px of the side columns; null = default. */
  widths: { nav: number | null; inspector: number | null };
  /** Whether the flow diagram pane is open. */
  diagram: boolean;
}

/** localStorage key of the layout. */
export const LAYOUT_STORAGE_KEY = "smart-review.tour.layout";

/** Default: every column shown, default widths, diagram hidden (brief §4.3). */
export const DEFAULT_LAYOUT: TourLayout = {
  hidden: { nav: false, main: false, inspector: false },
  full: null,
  widths: { nav: null, inspector: null },
  diagram: false,
};

/** Width of a hidden column's strip in px. */
export const STRIP_PX = 34;
/** Smallest widths in px while resizing (the centre keeps `MIN_MAIN_PX`). */
export const MIN_WIDTH_PX = { nav: 200, inspector: 300 } as const;
const MIN_MAIN_PX = 320;
const GUTTERS_PX = 12;

const isColumn = (value: unknown): value is Column => COLUMNS.includes(value as Column);
const width = (value: unknown) =>
  typeof value === "number" && Number.isFinite(value) && value > 0 ? value : null;

/** The stored layout; the default when storage is unavailable or holds something else. */
export function loadLayout(): TourLayout {
  try {
    const raw = JSON.parse(
      window.localStorage.getItem(LAYOUT_STORAGE_KEY) ?? "null",
    ) as Partial<TourLayout> | null;
    if (typeof raw !== "object" || raw === null) return DEFAULT_LAYOUT;
    const hidden = { ...DEFAULT_LAYOUT.hidden };
    for (const column of COLUMNS) hidden[column] = raw.hidden?.[column] === true;
    const layout: TourLayout = {
      hidden,
      full: isColumn(raw.full) ? raw.full : null,
      widths: { nav: width(raw.widths?.nav), inspector: width(raw.widths?.inspector) },
      diagram: raw.diagram === true,
    };
    return COLUMNS.some((c) => !layout.hidden[c]) ? layout : { ...layout, hidden: DEFAULT_LAYOUT.hidden };
  } catch {
    return DEFAULT_LAYOUT;
  }
}

/** Stores the layout; without storage it lives for this page only. */
export function saveLayout(layout: TourLayout): void {
  try {
    window.localStorage.setItem(LAYOUT_STORAGE_KEY, JSON.stringify(layout));
  } catch {
    // Storage unavailable: the in-memory layout still applies.
  }
}

/** Which columns are shown (not strips). */
export function shownColumns(layout: TourLayout): Record<Column, boolean> {
  return {
    nav: layout.full ? layout.full === "nav" : !layout.hidden.nav,
    main: layout.full ? layout.full === "main" : !layout.hidden.main,
    inspector: layout.full ? layout.full === "inspector" : !layout.hidden.inspector,
  };
}

/** Hides a column; leaves full screen first; never hides the last shown column. */
export function hideColumn(layout: TourLayout, column: Column): TourLayout {
  const hidden = { ...layout.hidden, [column]: true };
  if (COLUMNS.every((c) => hidden[c])) return layout;
  return { ...layout, full: null, hidden };
}

/** Shows a column (click on its strip); leaves full screen. */
export function showColumn(layout: TourLayout, column: Column): TourLayout {
  return { ...layout, full: null, hidden: { ...layout.hidden, [column]: false } };
}

/** Full screen on for a column, or off when it is already full screen. */
export function toggleFull(layout: TourLayout, column: Column): TourLayout {
  if (layout.full === column) return { ...layout, full: null };
  return { ...layout, full: column, hidden: { ...layout.hidden, [column]: false } };
}

/** A side column's width kept inside the workspace: the other side and the centre keep their room. */
export function clampWidth(column: "nav" | "inspector", value: number, total: number, other: number): number {
  const max = total - other - GUTTERS_PX - MIN_MAIN_PX;
  return Math.round(Math.max(MIN_WIDTH_PX[column], Math.min(value, max)));
}
