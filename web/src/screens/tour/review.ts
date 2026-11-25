// Helpers both review views (Flows and Files) share: keyboard guard, error kinds, top-bar facts, page scrolling.
import type { UseQueryResult } from "@tanstack/react-query";
import { useEffect, useState, type RefObject } from "react";
import { ApiError } from "../../api/client";
import type { PendingReview } from "../../api/generated/types";

/** The PR's checks page on GitHub, which the CI summary links to. */
export const checksHref = (pr: { html_url: string }) => `${pr.html_url.replace(/\/+$/, "")}/checks`;

/** Shortcuts are ignored while the user types or uses modifier keys. */
export function isTyping(event: KeyboardEvent): boolean {
  const target = event.target as HTMLElement | null;
  return (
    event.altKey ||
    event.ctrlKey ||
    event.metaKey ||
    target?.isContentEditable === true ||
    ["INPUT", "TEXTAREA", "SELECT"].includes(target?.tagName ?? "")
  );
}

/** Open the focused code line, or the current selection or step's first changed line. */
export function openGithubLine(
  main: HTMLElement | null,
  current: { side: "head" | "base"; line: number } | undefined,
) {
  const focused = document.activeElement?.closest("[data-line], [data-diff-line]");
  const active =
    focused && main?.contains(focused)
      ? focused.querySelector<HTMLAnchorElement>("[data-line-github]")
      : undefined;
  const row =
    current &&
    main?.querySelector(
      `[data-side="${current.side}"][data-line="${current.line}"], [data-ask-side="${current.side}"][data-ask-number="${current.line}"]`,
    );
  const fallback =
    row?.querySelector<HTMLAnchorElement>("[data-line-github]") ??
    main?.querySelector<HTMLAnchorElement>(
      "[data-line]:not([data-context]) [data-line-github], [data-diff-line] [data-line-github]",
    );
  (active ?? fallback)?.click();
}

/** The file section at the top edge of a multi-file Flows block. */
export function useVisibleCodePath(
  ref: RefObject<HTMLElement | null>,
  blockKey: string,
  mainVisible: boolean,
  fallback: string | undefined,
) {
  const [visible, setVisible] = useState<{ blockKey: string; path?: string }>();
  useEffect(() => {
    const scroller = ref.current;
    if (!scroller || !mainVisible) return;
    let frame = 0;
    const measure = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const top = scroller.getBoundingClientRect().top + 1;
        const section = [...scroller.querySelectorAll<HTMLElement>("[data-code-path]")].find((el) => {
          const rect = el.getBoundingClientRect();
          return rect.top <= top && rect.bottom > top;
        });
        const path = section?.dataset.codePath;
        setVisible((old) => (old?.blockKey === blockKey && old.path === path ? old : { blockKey, path }));
      });
    };
    const observer = new MutationObserver(measure);
    observer.observe(scroller, { subtree: true, childList: true });
    scroller.addEventListener("scroll", measure, { passive: true });
    window.addEventListener("resize", measure);
    measure();
    return () => {
      observer.disconnect();
      scroller.removeEventListener("scroll", measure);
      window.removeEventListener("resize", measure);
      cancelAnimationFrame(frame);
    };
  }, [blockKey, mainVisible, ref]);
  return (visible?.blockKey === blockKey ? visible.path : undefined) ?? fallback;
}

/** A failure the next try may not have: the server or the network, not an answer about this request. */
export const transient = (error: unknown) =>
  !(error instanceof ApiError) || error.status === 0 || error.status >= 500;

/** The top bar's pending-review count: the number (`n+` when incomplete), unavailable, or loading. */
export const pendingCountOf = (pending: UseQueryResult<PendingReview>) =>
  pending.isSuccess
    ? pending.data.commenting_available
      ? pending.data.incomplete
        ? (`${pending.data.comments.length}+` as const)
        : pending.data.comments.length
      : ("unavailable" as const)
    : pending.isPending
      ? ("loading" as const)
      : undefined;

/** The review screen never scrolls the page: no reserved scrollbar gutter, the columns get the full width. */
export function useNoPageScroll(): void {
  useEffect(() => {
    const style = document.documentElement.style;
    const before = style.scrollbarGutter;
    style.scrollbarGutter = "auto";
    return () => {
      style.scrollbarGutter = before;
    };
  }, []);
}
