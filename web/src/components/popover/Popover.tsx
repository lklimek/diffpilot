import {
  cloneElement,
  useEffect,
  useEffectEvent,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactElement,
  type ReactNode,
} from "react";
import { Tooltip, useTipPrecedence } from "../tooltip/Tooltip";

interface PopoverProps {
  /** Accessible name of the popover, e.g. "LLM remarks at line 91". */
  label: string;
  /** Provenance tag in the popover's top row, e.g. an "LLM" or "GitHub" label with its own tooltip. */
  tag?: ReactNode;
  /** The text; `pinned` adds what only a pinned popover offers (links). Also the trigger's description. */
  content: (pinned: boolean) => ReactNode;
  /** The trigger: one button without an `onClick`; it gets the ARIA state and the click. */
  children: ReactElement<{
    onClick?: () => void;
    "aria-expanded"?: boolean;
    "aria-haspopup"?: "dialog";
    "aria-controls"?: string;
    "aria-describedby"?: string;
  }>;
  /** Classes of the wrapper (layout of the trigger). */
  className?: string;
}

// One pinned popover on the whole page: pinning one closes any other.
let pinnedId: string | null = null;
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => listeners.delete(listener);
};
const setPinned = (id: string | null) => {
  pinnedId = id;
  for (const listener of listeners) listener();
};

const WIDTH = 360;
const ROOM = 200;

/**
 * A marker's popover: hover or keyboard focus shows it (WCAG 1.4.13: Escape dismisses, the pointer can move
 * onto it); a click, Enter or Space pins it with selectable text until its close button, Escape, a click
 * outside, or another pinned popover closes it. Closing by button or Escape returns the focus to the trigger.
 */
export function Popover({ label, tag, content, children, className = "" }: PopoverProps) {
  const id = useId();
  const wrapper = useRef<HTMLSpanElement>(null);
  const pinned = useSyncExternalStore(subscribe, () => pinnedId === id);
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  // Closed by Escape or the close button; shown again after the pointer or the focus left and came back.
  const [dismissed, setDismissed] = useState(false);
  const [position, setPosition] = useState<{ top?: number; bottom?: number; left: number }>({ left: 0 });
  const open = pinned || ((hovered || focused) && !dismissed);
  useTipPrecedence(hovered || focused);

  const trigger = () => wrapper.current?.firstElementChild as HTMLElement | null | undefined;
  const close = () => {
    if (pinnedId === id) setPinned(null);
    setDismissed(true);
    const active = document.activeElement;
    if (active === document.body || wrapper.current?.contains(active)) trigger()?.focus();
  };

  // Fixed position so scroll containers do not clip it; it follows its trigger while the page scrolls.
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const rect = trigger()?.getBoundingClientRect();
      if (!rect) return;
      const left = Math.max(4, Math.min(rect.left, window.innerWidth - WIDTH - 8));
      const above = rect.bottom + ROOM > window.innerHeight && rect.top > ROOM;
      setPosition(above ? { bottom: window.innerHeight - rect.top, left } : { top: rect.bottom, left });
    };
    place();
    window.addEventListener("scroll", place, true);
    window.addEventListener("resize", place);
    return () => {
      window.removeEventListener("scroll", place, true);
      window.removeEventListener("resize", place);
    };
  }, [open]);

  const onEscape = useEffectEvent(close);
  useEffect(() => {
    if (!pinned) return;
    const outside = (event: PointerEvent) => {
      if (!wrapper.current?.contains(event.target as Node) && pinnedId === id) setPinned(null);
    };
    // Capture phase: Escape closes the popover before page shortcuts see it.
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      onEscape();
    };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("keydown", escape, true);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("keydown", escape, true);
      if (pinnedId === id) setPinned(null);
    };
  }, [pinned, id]);

  return (
    <span
      ref={wrapper}
      className={className}
      onPointerEnter={() => setHovered(true)}
      onPointerLeave={() => {
        setHovered(false);
        if (!focused) setDismissed(false);
      }}
      onFocus={() => setFocused(true)}
      onBlur={(event) => {
        if (wrapper.current?.contains(event.relatedTarget as Node | null)) return;
        setFocused(false);
        setDismissed(false);
      }}
      onKeyDown={(event) => {
        if (event.key !== "Escape" || !open || pinned) return;
        event.preventDefault();
        setDismissed(true);
      }}
      // Clicks here belong to the marker, not to an LLM highlight around it.
      onClick={(event) => event.stopPropagation()}
    >
      {cloneElement(children, {
        "aria-expanded": pinned,
        "aria-haspopup": "dialog",
        "aria-controls": id,
        "aria-describedby": `${id}-body`,
        onClick: () => {
          setDismissed(false);
          setPinned(pinned ? null : id);
        },
      })}
      <span
        id={id}
        role="dialog"
        aria-label={label}
        hidden={!open}
        style={position}
        className={`fixed z-50 block w-max max-w-[min(360px,calc(100vw-16px))] cursor-auto pt-1 font-sans text-xs font-normal whitespace-normal normal-case select-text ${
          position.bottom !== undefined ? "pt-0 pb-1" : ""
        }`}
      >
        <span
          className={`block rounded-md border bg-surface px-2.5 py-1.5 text-left text-text shadow-tip ${
            pinned ? "border-accent" : "border-border"
          }`}
        >
          <span className="mb-1 flex min-h-6 items-center gap-2">
            {tag}
            {pinned && (
              <Tooltip tip="Closes this popover (Esc)." className="ml-auto">
                <button
                  type="button"
                  aria-label="Close"
                  onClick={close}
                  className="inline-flex size-6 cursor-pointer items-center justify-center rounded border-0 bg-transparent text-[14px] text-muted hover:bg-surface-2 hover:text-text"
                >
                  <span aria-hidden>×</span>
                </button>
              </Tooltip>
            )}
          </span>
          <span id={`${id}-body`} className="grid gap-1.5">
            {content(pinned)}
          </span>
        </span>
      </span>
    </span>
  );
}
