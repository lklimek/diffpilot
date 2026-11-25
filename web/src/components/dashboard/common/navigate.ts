import type { MouseEvent } from "react";
import { inApp } from "../../inApp";

/** Called with an in-app href instead of a full page load (the screen routes); absent = a normal link. */
export type Navigate = (href: string) => void;

/**
 * Click handler of an in-app link: a plain primary click goes to `onNavigate`; modified clicks (new tab, window)
 * keep the browser's behaviour.
 */
export const navigateOnClick =
  (href: string, onNavigate?: Navigate) =>
  (event: MouseEvent<HTMLAnchorElement>): void => {
    if (onNavigate) inApp(event, () => onNavigate(href));
  };
