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
import { ProvenanceTag, type ProvenanceSource } from "../section/ProvenanceTag";
import { keyboardModality } from "../tooltip/modality";
import { Tooltip, useTipPrecedence } from "../tooltip/Tooltip";
import { useOutsidePress } from "./useOutsidePress";

interface PopoverProps {
  /** Accessible name of the popover, e.g. "LLM remarks at line 91". */
  label: string;
  /** Source of the content, shown as a provenance tag in the popover's top row. */
  tag?: ProvenanceSource;
  /** The text; `pinned` adds what only a pinned popover offers (links, actions); `close` unpins it. Also the trigger's description while open. */
  content: (pinned: boolean, close: () => void) => ReactNode;
  /** The trigger: one button without an `onClick`; it gets the ARIA state and the click. */
  children: ReactElement<{
    onClick?: () => void;
    "aria-expanded"?: boolean;
    "aria-haspopup"?: "dialog";
    "aria-controls"?: string;
    "aria-describedby"?: string;
  }>;
  /** Short plain text that describes the trigger while the popover is closed (the content is not mounted then). */
  hint?: string;
  /** A click acts at once (a folded row): the popup only explains on hover or focus, and is never pinned. */
  onActivate?: () => void;
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
// Distance kept from the viewport edges; the popover's gap and border.
const MARGIN = 8;
const FRAME = 8;

// Whether `rect` lies outside a clipping (scrolling) ancestor of `element`.
function clippedOut(element: HTMLElement, rect: DOMRect): boolean {
  for (let parent = element.parentElement; parent; parent = parent.parentElement) {
    const { overflowX, overflowY } = getComputedStyle(parent);
    if (overflowX === "visible" && overflowY === "visible") continue;
    const box = parent.getBoundingClientRect();
    if (rect.bottom < box.top || rect.top > box.bottom || rect.right < box.left || rect.left > box.right) {
      return true;
    }
  }
  return false;
}

/**
 * A marker's popover: hover or keyboard focus shows it (WCAG 1.4.13: Escape dismisses, the pointer can move
 * onto it); a click, Enter or Space pins it with selectable text until its close button, Escape, a click
 * outside, or another pinned popover closes it. Closing by button or Escape returns the focus to the trigger.
 */
export function Popover({ label, tag, content, hint, onActivate, children, className = "" }: PopoverProps) {
  const id = useId();
  const wrapper = useRef<HTMLSpanElement>(null);
  const pinned = useSyncExternalStore(subscribe, () => pinnedId === id);
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  // Closed by Escape or the close button; shown again after the pointer or the focus left and came back.
  const [dismissed, setDismissed] = useState(false);
  const [position, setPosition] = useState<{
    top?: number;
    bottom?: number;
    left: number;
    maxHeight?: number;
  }>({ left: 0 });
  // The trigger is scrolled out of view: the popover is not drawn over other parts of the page.
  const [outside, setOutside] = useState(false);
  const dialog = useRef<HTMLSpanElement>(null);
  const box = useRef<HTMLSpanElement>(null);
  const open = pinned || ((hovered || focused) && !dismissed);
  useTipPrecedence(hovered || focused);

  const trigger = () => wrapper.current?.firstElementChild as HTMLElement | null | undefined;
  const close = () => {
    if (pinnedId === id) setPinned(null);
    setDismissed(true);
    const active = document.activeElement;
    if (active === document.body || wrapper.current?.contains(active)) trigger()?.focus();
  };

  // Unpins after an action in the popover; the focus stays where the action put it, unless it is still in the
  // popover that closes (then it returns to the trigger).
  const unpin = () => {
    if (pinnedId === id) setPinned(null);
    setDismissed(true);
    // The pointer was on the popup that is gone: it re-enters the wrapper to show the popup again.
    setHovered(false);
    const popup = document.getElementById(id);
    if (popup?.contains(document.activeElement))
      (popup.parentElement?.firstElementChild as HTMLElement).focus();
  };

  // Fixed position so scroll containers do not clip it; it follows its trigger while the page scrolls, below it
  // when it fits, else above, else on the larger side with its own scroll.
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const element = trigger();
      if (!element) return;
      const rect = element.getBoundingClientRect();
      setOutside(clippedOut(element, rect));
      const width = dialog.current?.offsetWidth || WIDTH;
      const left = Math.max(4, Math.min(rect.left, window.innerWidth - width - MARGIN));
      const needed = (box.current?.scrollHeight ?? 0) + FRAME;
      const below = window.innerHeight - rect.bottom - MARGIN;
      const above = rect.top - MARGIN;
      setPosition(
        needed <= below || below >= above
          ? { top: rect.bottom, left, maxHeight: below }
          : { bottom: window.innerHeight - rect.top, left, maxHeight: above },
      );
    };
    place();
    window.addEventListener("scroll", place, true);
    window.addEventListener("resize", place);
    return () => {
      window.removeEventListener("scroll", place, true);
      window.removeEventListener("resize", place);
    };
  }, [open, pinned]);

  useOutsidePress(pinned, wrapper, () => {
    if (pinnedId === id) setPinned(null);
  });
  const onEscape = useEffectEvent(close);
  useEffect(() => {
    if (!pinned) return;
    // Capture phase: Escape in the popover (or with no focus) closes only the popover; no other handler sees
    // it. With the focus elsewhere (a comment being written) the key belongs to that element.
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      const active = document.activeElement;
      if (active !== document.body && !wrapper.current?.contains(active)) return;
      event.preventDefault();
      event.stopPropagation();
      onEscape();
    };
    document.addEventListener("keydown", escape, true);
    return () => {
      document.removeEventListener("keydown", escape, true);
      if (pinnedId === id) setPinned(null);
    };
  }, [pinned, id]);

  return (
    <span
      ref={wrapper}
      className={className}
      onPointerEnter={() => {
        setHovered(true);
        // Dismissed popups come back when the pointer re-enters.
        setDismissed(false);
      }}
      onPointerLeave={() => {
        setHovered(false);
        if (!focused) setDismissed(false);
      }}
      // Only keyboard focus shows the popup: the focus a click or tap gives the trigger must not keep it open
      // after the closing click.
      onFocus={() => setFocused(keyboardModality())}
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
        "aria-expanded": open && !outside,
        "aria-haspopup": "dialog",
        // The dialog exists only while open, so the references are given only then.
        "aria-controls": open ? id : undefined,
        "aria-describedby": open ? `${id}-body` : hint ? `${id}-hint` : undefined,
        onClick: () => {
          if (onActivate) {
            setDismissed(true);
            onActivate();
            return;
          }
          setDismissed(false);
          setPinned(pinned ? null : id);
        },
      })}
      {!open && hint && (
        <span id={`${id}-hint`} className="sr-only">
          {hint}
        </span>
      )}
      {open && (
        <span
          ref={dialog}
          id={id}
          role="dialog"
          aria-label={label}
          tabIndex={-1}
          hidden={outside}
          style={position}
          className={`fixed z-50 flex w-max max-w-[min(360px,calc(100vw-16px))] cursor-auto flex-col pt-1 font-sans text-xs font-normal whitespace-normal normal-case outline-none select-text ${
            position.bottom !== undefined ? "pt-0 pb-1" : ""
          }`}
        >
          <span
            ref={box}
            data-popover-box
            className={`block min-h-0 overflow-y-auto overscroll-contain rounded-md border bg-surface px-2.5 py-1.5 text-left text-text shadow-tip [overflow-wrap:anywhere] ${
              pinned ? "border-accent" : "border-border"
            }`}
          >
            <span className="mb-1 flex min-h-6 items-center gap-2">
              {tag && <ProvenanceTag source={tag} focusable={pinned} />}
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
              {content(pinned, unpin)}
            </span>
          </span>
        </span>
      )}
    </span>
  );
}
