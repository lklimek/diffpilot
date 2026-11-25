import type { MouseEvent } from "react";

/** Follow a plain primary click in the app, leaving modified clicks to the browser. */
export function inApp(event: MouseEvent, follow: () => void) {
  if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  event.preventDefault();
  follow();
}
