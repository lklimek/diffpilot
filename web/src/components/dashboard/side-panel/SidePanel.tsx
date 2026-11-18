import { useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import { Tooltip } from "../../tooltip/Tooltip";

/** localStorage key of the side panel: `{ open, width }`. */
export const SIDE_PANEL_STORAGE_KEY = "smart-review.dashboard.side-panel";
/** Below this viewport width the panel starts collapsed (OD5). */
export const SIDE_PANEL_OPEN_FROM_PX = 1600;
const MIN = 300;
const MAX = 900;
const STEP = 24;
const DEFAULT_WIDTH = "clamp(340px,22vw,560px)";

interface Stored {
  open: boolean | null;
  width: number | null;
}

function load(): Stored {
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(SIDE_PANEL_STORAGE_KEY) ?? "null");
    if (typeof value !== "object" || value === null) return { open: null, width: null };
    const { open, width } = value as Record<string, unknown>;
    return {
      open: typeof open === "boolean" ? open : null,
      width: typeof width === "number" && width >= MIN && width <= MAX ? width : null,
    };
  } catch {
    return { open: null, width: null };
  }
}

function save(state: Stored): void {
  try {
    window.localStorage.setItem(SIDE_PANEL_STORAGE_KEY, JSON.stringify(state));
  } catch {
    // Storage unavailable: the state lives for this page only.
  }
}

/** Width of DEFAULT_WIDTH at the current viewport. */
const defaultWidth = () => Math.round(Math.max(340, Math.min(560, window.innerWidth * 0.22)));

const wide = () => window.matchMedia?.(`(min-width: ${SIDE_PANEL_OPEN_FROM_PX}px)`).matches ?? true;

interface SidePanelProps {
  /** The main column. */
  main: ReactNode;
  /** The panel's sections. */
  children: ReactNode;
  /** Text of the collapsed rail, e.g. `More · waiting on others 3`. */
  railText: string;
}

/**
 * Main column plus a secondary side panel (mockup `.dash`, `renderAside`): collapsed below 1600 px unless the
 * user opened it; resizable by drag or arrow keys on the splitter (double-click resets); both remembered.
 */
export function SidePanel({ main, children, railText }: SidePanelProps) {
  const [state, setState] = useState<Stored>(load);
  const aside = useRef<HTMLElement>(null);
  const open = state.open ?? wide();
  const update = (next: Partial<Stored>) =>
    setState((current) => {
      const merged = { ...current, ...next };
      save(merged);
      return merged;
    });
  const resize = (px: number) => update({ width: Math.round(Math.max(MIN, Math.min(MAX, px))) });
  const current = () => aside.current?.offsetWidth || state.width || defaultWidth();

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    resize(current() + (event.key === "ArrowLeft" ? STEP : -STEP));
  };
  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    const handle = event.currentTarget;
    handle.setPointerCapture?.(event.pointerId);
    const x0 = event.clientX;
    const w0 = current();
    const move = (e: globalThis.PointerEvent) => resize(w0 - (e.clientX - x0));
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
  };

  return (
    <div
      className="grid items-start gap-x-3.5"
      style={{
        gridTemplateColumns: open
          ? `minmax(0,1fr) 8px ${state.width ? `${state.width}px` : DEFAULT_WIDTH}`
          : "minmax(0,1fr) 36px",
      }}
    >
      <div className="min-w-0">{main}</div>
      {open && (
        <Tooltip
          tip="Drag or use the arrow keys to resize the side panel; double-click resets"
          className="sticky top-16 block h-[calc(100vh-96px)] min-h-[200px]"
        >
          <div
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize the side panel"
            aria-valuemin={MIN}
            aria-valuemax={MAX}
            aria-valuenow={state.width ?? defaultWidth()}
            tabIndex={0}
            onKeyDown={onKeyDown}
            onPointerDown={onPointerDown}
            onDoubleClick={() => update({ width: null })}
            className="block h-full w-2 cursor-col-resize touch-none rounded bg-border hover:bg-accent focus-visible:bg-accent"
          />
        </Tooltip>
      )}
      <aside
        ref={aside}
        aria-label="More: waiting on others, yesterday, charts, scan notes"
        className="sticky top-16 grid max-h-[calc(100vh-80px)] min-w-0 content-start gap-2.5 overflow-auto"
      >
        {open ? (
          <>
            <div className="flex items-center gap-2">
              <h2 className="flex-1 text-[13px] font-semibold tracking-wide text-muted uppercase">More</h2>
              <Tooltip tip="Collapse the side panel">
                <button
                  type="button"
                  aria-expanded="true"
                  aria-label="Collapse the side panel"
                  onClick={() => update({ open: false })}
                  className="h-7 min-w-7 cursor-pointer rounded-md border border-border bg-surface px-1.5 font-semibold hover:border-accent"
                >
                  »
                </button>
              </Tooltip>
            </div>
            {children}
          </>
        ) : (
          <Tooltip tip="Open the side panel: waiting on others, yesterday, charts, scan notes">
            <button
              type="button"
              aria-expanded="false"
              onClick={() => update({ open: true })}
              className="flex min-h-[280px] w-9 cursor-pointer items-center gap-2 rounded-lg border border-border bg-surface py-3 text-[13px] font-semibold text-text [writing-mode:vertical-rl] hover:border-accent hover:bg-accent-bg"
            >
              « {railText}
            </button>
          </Tooltip>
        )}
      </aside>
    </div>
  );
}
