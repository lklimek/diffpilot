// Keys of the question cards (plan §3.8): 1–8 choose, Enter decide, a Ask, s Skip.
import { useEffect, useRef } from "react";

/** A field that takes typed keys (radios and checkboxes leave 1–8, Enter, a and s to the card). */
export const isTypingField = (target: EventTarget | null) =>
  target instanceof HTMLElement &&
  (/^(TEXTAREA|SELECT)$/.test(target.tagName) ||
    target.isContentEditable ||
    (target instanceof HTMLInputElement && !["radio", "checkbox"].includes(target.type)));

/** A refreshed card leaves keyboard focus with the reader while Queue Status is in use. */
export function focusQuestionHeading(heading: HTMLElement | null) {
  if (!document.activeElement?.closest("[data-queue-status]")) heading?.focus();
}

/** What a card's keys do; a key without its handler does nothing. */
export interface CardKeys {
  /** Number of options 1–8 choose from. */
  options?: number;
  /** Chooses option `index` (0-based). */
  choose?: (index: number) => void;
  decide: () => void;
  ask?: () => void;
  skip: () => void;
}

/** Listens for the card's keys while focus is not in a field, the Ask chat or a dialog. */
export function useCardKeys(keys: CardKeys): void {
  const latest = useRef(keys);
  useEffect(() => {
    latest.current = keys;
  });
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        event.ctrlKey ||
        event.metaKey ||
        event.altKey ||
        isTypingField(event.target)
      )
        return;
      if ((event.target as HTMLElement | null)?.closest?.("[data-ask-chat], [role=dialog], [role=menu]"))
        return;
      const { options = 0, choose, decide, ask, skip } = latest.current;
      const n = Number(event.key);
      if (choose && Number.isInteger(n) && n >= 1 && n <= Math.min(8, options)) {
        event.preventDefault();
        choose(n - 1);
      } else if (
        event.key === "Enter" &&
        !(event.target instanceof HTMLButtonElement || event.target instanceof HTMLAnchorElement)
      ) {
        event.preventDefault();
        decide();
      } else if (event.key === "a" && ask) {
        event.preventDefault();
        ask();
      } else if (event.key === "s") {
        event.preventDefault();
        skip();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);
}
