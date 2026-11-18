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
}

const TIP_WIDTH = 320;
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
export function Tooltip({ tip, children, className = "", placement = "below" }: TooltipProps) {
  const id = useId();
  const wrapper = useRef<HTMLSpanElement>(null);
  // Fixed position so scroll containers (code view, columns) do not clip the tip.
  const [position, setPosition] = useState<{ top?: number; bottom?: number; left: number } | null>(null);
  // Set by a press of the pointer until it leaves the trigger: no tip from that hover or that focus.
  const pressed = useRef(false);
  // Nested triggers showing their own tips; while any does, this tip stays hidden.
  const [covered, setCovered] = useState(0);
  const cover = useCallback((delta: 1 | -1) => setCovered((n) => n + delta), []);
  useTipPrecedence(position !== null);
  const show = () => {
    const rect = wrapper.current?.getBoundingClientRect();
    if (!rect || pressed.current) return;
    if (placement === "right" && rect.right + 8 + TIP_WIDTH <= window.innerWidth) {
      setPosition({ top: rect.top, left: rect.right + 8 });
      return;
    }
    const left = Math.max(4, Math.min(rect.left, window.innerWidth - TIP_WIDTH - 8));
    const above = rect.bottom + TIP_ROOM > window.innerHeight && rect.top > TIP_ROOM;
    setPosition(above ? { bottom: window.innerHeight - rect.top + 4, left } : { top: rect.bottom + 4, left });
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
      }}
    >
      <Enclosing.Provider value={cover}>
        {cloneElement(children, {
          "aria-describedby": [children.props["aria-describedby"], id].filter(Boolean).join(" "),
        })}
      </Enclosing.Provider>
      <span
        id={id}
        role="tooltip"
        hidden={position === null || covered > 0}
        style={position ?? undefined}
        className="fixed z-50 w-max max-w-80 rounded-md bg-text px-2.5 py-1.5 text-left font-sans text-xs font-normal whitespace-normal text-bg normal-case shadow-tip"
      >
        {tip}
      </span>
    </span>
  );
}
