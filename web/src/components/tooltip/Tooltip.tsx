import {
  cloneElement,
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactElement,
  type ReactNode,
} from "react";

interface TooltipProps {
  /** Tooltip content; also the accessible description of the trigger. */
  tip: ReactNode;
  /** The trigger: one focusable element (a non-interactive one needs `tabIndex={0}`). */
  children: ReactElement<{ "aria-describedby"?: string }>;
  /** Classes of the wrapper (layout of the trigger). */
  className?: string;
  /** `below` (default; above when there is no room) or `right` of the trigger, e.g. for rows of a list. */
  placement?: "below" | "right";
  /** Long statements: constrain to the viewport and scroll with arrow / Page Up / Page Down keys. */
  scrollable?: boolean;
}

const TIP_WIDTH = 320;
const TIP_GUTTER = 16;
// Room a tip may need under its trigger; with less, it opens above.
const TIP_ROOM = 140;

// Lets a trigger inside a tooltip's trigger hide the outer tip while its own takes over (+1 / -1).
const Enclosing = createContext<((delta: 1 | -1) => void) | null>(null);

/** Hides the tips of enclosing tooltips while `active`: the inner trigger's own tip takes precedence. */
export function useTipPrecedence(active: boolean): void {
  const enclosing = useContext(Enclosing);
  useEffect(() => {
    if (!active || !enclosing) return;
    enclosing(1);
    return () => enclosing(-1);
  }, [active, enclosing]);
}

/**
 * Shows `tip` while the trigger is hovered or has keyboard focus (WCAG 1.4.13): Escape dismisses it, the pointer
 * can move onto it, and it stays until then. A click closes it and the focus the click gives does not reopen it,
 * so it never covers the element the pointer goes to next; it opens again when the pointer comes back.
 */
export function Tooltip({
  tip,
  children,
  className = "",
  placement = "below",
  scrollable = false,
}: TooltipProps) {
  const id = useId();
  const wrapper = useRef<HTMLSpanElement>(null);
  const tipElement = useRef<HTMLSpanElement>(null);
  // Fixed position so scroll containers (code view, columns) do not clip the tip.
  const [position, setPosition] = useState<{
    top?: number;
    bottom?: number;
    left: number;
    maxHeight?: number;
  } | null>(null);
  // Set by a press of the pointer until it leaves the trigger: no tip from that hover or that focus.
  const pressed = useRef(false);
  // Nested triggers showing their own tips; while any does, this tip stays hidden.
  const [covered, setCovered] = useState(0);
  const cover = useCallback((delta: 1 | -1) => setCovered((n) => n + delta), []);
  useTipPrecedence(position !== null);
  const show = () => {
    const rect = wrapper.current?.getBoundingClientRect();
    if (!rect || pressed.current) return;
    const width = Math.min(TIP_WIDTH, window.innerWidth - 2 * TIP_GUTTER);
    if (placement === "right" && rect.right + 8 + width <= window.innerWidth - TIP_GUTTER) {
      setPosition({ top: rect.top, left: rect.right + 8 });
      return;
    }
    const left = Math.max(TIP_GUTTER, Math.min(rect.left, window.innerWidth - width - TIP_GUTTER));
    const above = scrollable
      ? rect.top > window.innerHeight - rect.bottom
      : rect.bottom + TIP_ROOM > window.innerHeight && rect.top > TIP_ROOM;
    const maxHeight = scrollable
      ? Math.max(
          0,
          Math.min(512, window.innerHeight * 0.6, (above ? rect.top : window.innerHeight - rect.bottom) - 12),
        )
      : undefined;
    setPosition(
      above
        ? { bottom: window.innerHeight - rect.top + 4, left, maxHeight }
        : { top: rect.bottom + 4, left, maxHeight },
    );
  };
  const hide = () => setPosition(null);
  return (
    <span
      ref={wrapper}
      className={className}
      onPointerEnter={show}
      onPointerLeave={() => {
        pressed.current = false;
        hide();
      }}
      onPointerDown={() => {
        pressed.current = true;
        hide();
      }}
      onFocus={show}
      onBlur={() => {
        pressed.current = false;
        hide();
      }}
      onKeyDown={(event) => {
        if (event.key === "Escape") hide();
        const tip = tipElement.current;
        if (!scrollable || position === null || !tip || tip.scrollHeight <= tip.clientHeight) return;
        const offsets: Partial<Record<string, number>> = {
          ArrowDown: 40,
          ArrowUp: -40,
          PageDown: tip.clientHeight,
          PageUp: -tip.clientHeight,
        };
        const distance = offsets[event.key];
        if (distance !== undefined) {
          event.preventDefault();
          tip.scrollBy({ top: distance });
        }
      }}
    >
      <Enclosing.Provider value={cover}>
        {cloneElement(children, {
          "aria-describedby": [children.props["aria-describedby"], id].filter(Boolean).join(" "),
        })}
      </Enclosing.Provider>
      <span
        ref={tipElement}
        id={id}
        role="tooltip"
        hidden={position === null || covered > 0}
        style={position ?? undefined}
        className={`fixed z-50 w-max max-w-[min(20rem,calc(100vw-32px))] rounded-md bg-text px-2.5 py-1.5 text-left font-sans text-xs font-normal whitespace-normal text-bg normal-case shadow-tip ${scrollable ? "overflow-y-auto" : ""}`}
      >
        {tip}
      </span>
    </span>
  );
}
