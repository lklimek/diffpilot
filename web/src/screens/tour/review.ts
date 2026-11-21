// Helpers both review views (Flows and Files) share: keyboard guard, error kinds, top-bar facts, page scrolling.
import type { UseQueryResult } from "@tanstack/react-query";
import { useEffect } from "react";
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
