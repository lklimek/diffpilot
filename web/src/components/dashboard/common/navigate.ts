import type { MouseEvent } from "react";

/** Called with an in-app href instead of a full page load (the screen routes); absent = a normal link. */
export type Navigate = (href: string) => void;

/**
 * Click handler of an in-app link: a plain primary click goes to `onNavigate`; modified clicks (new tab, window)
 * keep the browser's behaviour.
 */
export const navigateOnClick =
  (href: string, onNavigate?: Navigate) =>
  (event: MouseEvent<HTMLAnchorElement>): void => {
    if (
      !onNavigate ||
      event.button !== 0 ||
      event.metaKey ||
      event.ctrlKey ||
      event.shiftKey ||
      event.altKey
    ) {
      return;
    }
    event.preventDefault();
    onNavigate(href);
  };
