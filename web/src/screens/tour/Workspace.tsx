import {
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { HEAD_BUTTON } from "../../components/button/button";
import { Tooltip } from "../../components/tooltip/Tooltip";
import {
  MIN_WIDTH_PX,
  STRIP_PX,
  canHide,
  clampWidth,
  defaultWidth,
  hideColumn,
  showColumn,
  shownColumns,
  sideWidths,
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
const DEFAULT_WIDTH = { nav: "clamp(240px, 17vw, 400px)", inspector: "clamp(400px, 27vw, 620px)" } as const;

/** Width of the element in px, kept current on resize; 0 until measured. */
function useWidth(ref: RefObject<HTMLElement | null>): number {
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setWidth(el.getBoundingClientRect().width);
    measure();
    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", measure);
      return () => window.removeEventListener("resize", measure);
    }
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);
  return width;
}
const KEY_STEP_PX = 24;

function ColumnHead({
  column,
  layout,
  onChange,
}: { column: Column } & Pick<WorkspaceProps, "layout" | "onChange">) {
  const full = layout.full === column;
  const hideable = canHide(layout, column);
  return (
    <div className="flex flex-none items-center gap-1.5 border-b border-border bg-surface-2 px-2.5 py-1">
      <h2 data-column-title className="m-0 flex-1 text-[13px] font-bold">
        {TITLE[column]}
      </h2>
      <Tooltip
        tip={
          hideable
            ? "Hide this column; click its strip to show it again"
            : "At least one column stays visible"
        }
      >
        <button
          type="button"
          aria-label={`Hide ${TITLE[column]} column`}
          aria-disabled={!hideable || undefined}
          onClick={() => hideable && onChange(hideColumn(layout, column))}
          className={`${HEAD_BUTTON} aria-disabled:cursor-not-allowed aria-disabled:opacity-60`}
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
          className={HEAD_BUTTON}
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
  const total = useWidth(grid);
  const shown = shownColumns(layout);
  // Until the workspace is measured (first paint, or no layout engine) the CSS defaults apply.
  const drawn = total > 0 ? sideWidths(layout, total) : null;
  const side = (column: "nav" | "inspector") =>
    !shown[column]
      ? `${STRIP_PX}px`
      : !shown.main
        ? "minmax(0,1fr)"
        : drawn
          ? `${drawn[column]}px`
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
        aria-valuenow={drawn?.[column] ?? layout.widths[column] ?? defaultWidth(column, window.innerWidth)}
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
    <div
      ref={grid}
      className="grid min-h-0 flex-1 grid-rows-[minmax(0,1fr)] overflow-hidden"
      style={{ gridTemplateColumns: template }}
    >
      <nav
        data-column="nav"
        aria-label="Flows and steps"
        className="relative flex min-h-0 min-w-0 flex-col overflow-hidden border-r border-border bg-surface"
      >
        {body(
          "nav",
          <div tabIndex={0} className="relative min-h-0 flex-1 overflow-auto overscroll-contain p-3">
            {nav}
          </div>,
        )}
      </nav>
      {gutter("nav")}
      <main
        id="main"
        data-column="main"
        tabIndex={-1}
        className="relative flex min-h-0 min-w-0 flex-col overflow-hidden bg-bg"
      >
        {body(
          "main",
          <div
            ref={mainRef}
            tabIndex={0}
            data-scroll-column
            className="relative min-h-0 flex-1 overflow-auto overscroll-contain px-5 pt-3 pb-10"
          >
            {main}
          </div>,
        )}
      </main>
      {gutter("inspector")}
      <aside
        data-column="inspector"
        aria-label="Inspector"
        className="relative flex min-h-0 min-w-0 flex-col overflow-hidden border-l border-border bg-surface"
      >
        {body("inspector", <div className="relative min-h-0 flex-1">{inspector}</div>)}
      </aside>
    </div>
  );
}
