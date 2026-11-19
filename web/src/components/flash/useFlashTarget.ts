import { useEffect, useEffectEvent, type RefObject } from "react";

/** How long a flashed target keeps its frame (its tint fades over the same time, see `index.css`). */
export const FLASH_MS = 3000;

// About one second of frames to wait for a target that is still rendering or collapsed.
const MAX_FRAMES = 60;

/** What to flash: a CSS selector inside the container; a new `nonce` flashes the same target again. */
export interface FlashRequest {
  selector: string;
  nonce: number;
}

/**
 * Scroll to and briefly highlight a block: the element matching `request.selector` inside `container` scrolls
 * into view and gets `data-flash` (frame plus a fading amber tint, `index.css`) for `FLASH_MS`. Waits a moment
 * for a target that is not shown yet (a section opening); with reduced motion it jumps and only frames.
 * `request`: null or undefined does nothing. `onDone` runs once the request is handled: its frame ended, or no
 * target showed. No attention tracking.
 */
export function useFlashTarget(
  container: RefObject<HTMLElement | null>,
  request: FlashRequest | null | undefined,
  onDone?: () => void,
): void {
  const selector = request?.selector;
  const nonce = request?.nonce;
  const done = useEffectEvent(() => onDone?.());
  useEffect(() => {
    if (selector === undefined) return;
    let frames = 0;
    let frame = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let flashed: HTMLElement | null = null;
    const attempt = () => {
      const target = container.current?.querySelector<HTMLElement>(selector);
      if (!target || target.closest("[hidden]")) {
        if (++frames < MAX_FRAMES) frame = requestAnimationFrame(attempt);
        else done();
        return;
      }
      const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
      target.scrollIntoView?.({ block: "center", inline: "nearest", behavior: reduce ? "auto" : "smooth" });
      target.setAttribute("data-flash", "");
      flashed = target;
      timer = setTimeout(() => {
        target.removeAttribute("data-flash");
        flashed = null;
        done();
      }, FLASH_MS);
    };
    frame = requestAnimationFrame(attempt);
    return () => {
      cancelAnimationFrame(frame);
      clearTimeout(timer);
      flashed?.removeAttribute("data-flash");
    };
  }, [container, selector, nonce]);
}
