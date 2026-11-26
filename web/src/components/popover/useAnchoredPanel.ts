import {
  useLayoutEffect,
  useRef,
  useState,
  type FocusEvent,
  type KeyboardEvent,
  type RefObject,
} from "react";
import { useOutsidePress } from "./useOutsidePress";

interface Options {
  align?: "start" | "end";
  gap?: number;
  anchorRef?: RefObject<HTMLButtonElement | null>;
}

/** Keep a non-modal panel inside the viewport and dismiss it when focus or a pointer leaves. */
export function useAnchoredPanel<P extends HTMLElement>({
  align = "start",
  gap = 4,
  anchorRef,
}: Options = {}) {
  const root = useRef<HTMLDivElement>(null);
  const ownAnchor = useRef<HTMLButtonElement>(null);
  const anchor = anchorRef ?? ownAnchor;
  const panel = useRef<P>(null);
  const [open, setOpen] = useState(false);
  useOutsidePress(open, root, () => setOpen(false));

  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      if (!anchor.current || !panel.current) return;
      const { left, right, top, bottom } = anchor.current.getBoundingClientRect();
      const margin = 8;
      const above = top - gap - margin;
      const below = window.innerHeight - bottom - gap - margin;
      const opensAbove = panel.current.scrollHeight > below && above > below;
      const width = panel.current.offsetWidth;
      const x = align === "end" ? right - width : left;
      Object.assign(panel.current.style, {
        left: `${Math.max(margin, Math.min(x, window.innerWidth - width - margin))}px`,
        top: opensAbove ? "" : `${bottom + gap}px`,
        bottom: opensAbove ? `${window.innerHeight - top + gap}px` : "",
        maxHeight: `${Math.max(0, opensAbove ? above : below)}px`,
      });
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [align, anchor, gap, open]);

  const closeAndFocus = () => {
    setOpen(false);
    anchor.current?.focus();
  };
  return {
    root,
    anchor,
    panel,
    open,
    setOpen,
    closeAndFocus,
    dismiss: {
      onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => {
        if (!open || event.key !== "Escape") return;
        event.preventDefault();
        event.stopPropagation();
        closeAndFocus();
      },
      onBlur: (event: FocusEvent<HTMLDivElement>) => {
        if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget)) setOpen(false);
      },
    },
  };
}
