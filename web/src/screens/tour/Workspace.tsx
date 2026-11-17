import { useRef, type KeyboardEvent, type PointerEvent, type ReactNode, type RefObject } from "react";
import { Tooltip } from "../../components/tooltip/Tooltip";
import {
  MIN_WIDTH_PX,
  STRIP_PX,
  clampWidth,
  hideColumn,
  showColumn,
  shownColumns,
  toggleFull,
  type Column,
  type TourLayout,
} from "./layout";

interface WorkspaceProps {
  layout: TourLayout;
  /** Called with every layout change (hide, show, full screen, resize). */
  onChange: (layout: TourLayout) => void;
  nav: ReactNode;
  main: ReactNode;
  inspector: ReactNode;
  /** The scrolling centre (scrolled to the top on a new block). */
  mainRef: RefObject<HTMLDivElement | null>;
}

const TITLE: Record<Column, string> = { nav: "Flows", main: "Review", inspector: "Inspector" };
const DEFAULT_WIDTH = { nav: "var(--width-nav)", inspector: "clamp(370px, 27vw, 620px)" } as const;
/** The default widths of `DEFAULT_WIDTH` in px, for the separators' value. */
const defaultPx = (column: "nav" | "inspector") =>
  column === "nav"
    ? Math.max(270, Math.min(400, Math.round(window.innerWidth * 0.17)))
    : Math.max(370, Math.min(620, Math.round(window.innerWidth * 0.27)));
const KEY_STEP_PX = 24;
const SMALL_BUTTON =
  "cursor-pointer rounded border border-border bg-surface px-1.5 text-[11.5px] text-text hover:border-accent aria-pressed:border-accent aria-pressed:bg-accent-bg";

function ColumnHead({
  column,
  layout,
  onChange,
}: { column: Column } & Pick<WorkspaceProps, "layout" | "onChange">) {
  const full = layout.full === column;
  return (
    <div className="flex flex-none items-center gap-1.5 border-b border-border bg-surface-2 px-2.5 py-1">
      <span className="flex-1 text-[13px] font-bold">{TITLE[column]}</span>
      <Tooltip tip="Hide this column; click its strip to show it again">
        <button
          type="button"
          aria-label={`Hide ${TITLE[column]} column`}
          onClick={() => onChange(hideColumn(layout, column))}
          className={SMALL_BUTTON}
        >
          Hide
        </button>
      </Tooltip>
      <Tooltip tip="Show only this column (click again or press Esc to return)">
        <button
          type="button"
          aria-pressed={full}
          aria-label={`Full screen ${TITLE[column]} column`}
          onClick={() => onChange(toggleFull(layout, column))}
          className={SMALL_BUTTON}
        >
          {full ? "Exit full screen" : "Full screen"}
        </button>
      </Tooltip>
    </div>
  );
}

function Strip({
  column,
  layout,
  onChange,
}: { column: Column } & Pick<WorkspaceProps, "layout" | "onChange">) {
  return (
    <Tooltip tip={`Click to show the ${TITLE[column]} column`} className="absolute inset-0">
      <button
        type="button"
        data-strip={column}
        aria-label={`Show ${TITLE[column]} column`}
        onClick={() => onChange(showColumn(layout, column))}
        className="flex h-full w-full cursor-pointer items-start justify-center border-0 bg-surface-2 py-3.5 text-[13px] font-semibold text-text hover:bg-accent-bg"
      >
        <span className="[writing-mode:vertical-rl]">{TITLE[column]}</span>
      </button>
    </Tooltip>
  );
}

/** The three columns of the review screen: hide, full screen, strips that restore, draggable widths. */
export function Workspace({ layout, onChange, nav, main, inspector, mainRef }: WorkspaceProps) {
  const grid = useRef<HTMLDivElement>(null);
  const dragging = useRef<"nav" | "inspector" | null>(null);
  const shown = shownColumns(layout);
  const side = (column: "nav" | "inspector") =>
    !shown[column]
      ? `${STRIP_PX}px`
      : !shown.main
        ? "minmax(0,1fr)"
        : layout.widths[column] !== null
          ? `${layout.widths[column]}px`
          : DEFAULT_WIDTH[column];
  const template = [
    side("nav"),
    shown.nav && shown.main ? "6px" : "0px",
    shown.main ? "minmax(0,1fr)" : `${STRIP_PX}px`,
    shown.inspector && shown.main ? "6px" : "0px",
    side("inspector"),
  ].join(" ");

  // Measured in event handlers only.
  const widthOf = (column: Column) =>
    grid.current?.querySelector(`[data-column=${column}]`)?.getBoundingClientRect().width ?? 0;
  const resize = (column: "nav" | "inspector", width: number | null) => {
    const total = grid.current?.getBoundingClientRect().width ?? 0;
    const other = widthOf(column === "nav" ? "inspector" : "nav");
    const next = width === null ? null : clampWidth(column, width, total, other);
    onChange({ ...layout, widths: { ...layout.widths, [column]: next } });
  };
  const gutter = (column: "nav" | "inspector") => {
    if (!shown[column] || !shown.main) return <div aria-hidden />;
    return (
      <div
        role="separator"
        aria-orientation="vertical"
        tabIndex={0}
        aria-label={`Resize the ${TITLE[column]} column: drag, or use the arrow keys; double-click resets`}
        aria-valuenow={layout.widths[column] ?? defaultPx(column)}
        aria-valuemin={MIN_WIDTH_PX[column]}
        onPointerDown={(event: PointerEvent<HTMLDivElement>) => {
          dragging.current = column;
          event.currentTarget.setPointerCapture?.(event.pointerId);
          event.preventDefault();
        }}
        onPointerMove={(event: PointerEvent<HTMLDivElement>) => {
          if (dragging.current !== column || !grid.current) return;
          const rect = grid.current.getBoundingClientRect();
          resize(column, column === "nav" ? event.clientX - rect.left : rect.right - event.clientX);
        }}
        onPointerUp={() => (dragging.current = null)}
        onPointerCancel={() => (dragging.current = null)}
        onDoubleClick={() => resize(column, null)}
        onKeyDown={(event: KeyboardEvent<HTMLDivElement>) => {
          if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
          event.preventDefault();
          const wider = (event.key === "ArrowRight") === (column === "nav");
          resize(column, widthOf(column) + (wider ? KEY_STEP_PX : -KEY_STEP_PX));
        }}
        className="cursor-col-resize touch-none bg-border hover:bg-accent focus-visible:bg-accent"
      />
    );
  };
  const body = (column: Column, content: ReactNode) =>
    shown[column] ? (
      <>
        <ColumnHead column={column} layout={layout} onChange={onChange} />
        {content}
      </>
    ) : (
      <Strip column={column} layout={layout} onChange={onChange} />
    );
  return (
    <div ref={grid} className="grid min-h-0 flex-1" style={{ gridTemplateColumns: template }}>
      <nav
        data-column="nav"
        aria-label="Flows and steps"
        className="relative flex min-h-0 min-w-0 flex-col border-r border-border bg-surface"
      >
        {body("nav", <div className="min-h-0 flex-1 overflow-auto p-3">{nav}</div>)}
      </nav>
      {gutter("nav")}
      <main
        id="main"
        data-column="main"
        tabIndex={-1}
        className="relative flex min-h-0 min-w-0 flex-col bg-bg"
      >
        {body(
          "main",
          <div ref={mainRef} className="min-h-0 flex-1 overflow-auto px-5 pt-3 pb-10">
            {main}
          </div>,
        )}
      </main>
      {gutter("inspector")}
      <aside
        data-column="inspector"
        aria-label="Inspector"
        className="relative flex min-h-0 min-w-0 flex-col border-l border-border bg-surface"
      >
        {body("inspector", <div className="min-h-0 flex-1">{inspector}</div>)}
      </aside>
    </div>
  );
}
