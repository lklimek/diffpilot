import type { ReactNode } from "react";
import type { ItemKind } from "../../../api/generated/types";

const svg = (path: ReactNode, className = "h-3.5 w-3.5") => (
  <svg
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth={2}
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden
    className={`flex-none ${className}`}
  >
    {path}
  </svg>
);

const KIND_PATH: Record<ItemKind, ReactNode> = {
  pr: (
    <>
      <circle cx="6" cy="6" r="2.5" />
      <circle cx="6" cy="18" r="2.5" />
      <circle cx="18" cy="18" r="2.5" />
      <path d="M6 8.5v7M18 15.5V9a3 3 0 0 0-3-3h-4M13 3.5 10.5 6 13 8.5" />
    </>
  ),
  issue: (
    <>
      <circle cx="12" cy="12" r="9" />
      <circle cx="12" cy="12" r="1.6" fill="currentColor" />
    </>
  ),
  todo: (
    <>
      <rect x="4" y="4" width="16" height="16" rx="3" />
      <path d="M8 12l3 3 5-6" />
    </>
  ),
};

/** Icon of an item kind; unknown kinds (tolerant reader) get none. */
export const KindIcon = ({ kind }: { kind: ItemKind }) => (KIND_PATH[kind] ? svg(KIND_PATH[kind]) : null);

/** "Opens in a new tab" arrow. */
export const ExternalIcon = () =>
  svg(
    <path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5" />,
    "h-[0.8em] w-[0.8em]",
  );

/** Copy (two sheets). */
export const CopyIcon = () =>
  svg(
    <>
      <rect x="9" y="9" width="11" height="11" rx="2" />
      <path d="M5 15V6a2 2 0 0 1 2-2h8" />
    </>,
    "h-[0.8em] w-[0.8em]",
  );

/** Refresh arrow. */
export const RefreshIcon = () => svg(<path d="M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7" />, "h-[15px] w-[15px]");

/** Left-nav icons. */
export const NavIcon = ({ name }: { name: "my-work" | "review" | "settings" | "collapse" | "expand" }) =>
  svg(
    {
      "my-work": (
        <>
          <rect x="3" y="3" width="7" height="9" rx="1" />
          <rect x="14" y="3" width="7" height="5" rx="1" />
          <rect x="14" y="12" width="7" height="9" rx="1" />
          <rect x="3" y="16" width="7" height="5" rx="1" />
        </>
      ),
      review: KIND_PATH.pr,
      settings: (
        <>
          <circle cx="12" cy="12" r="3" />
          <path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9 7 7M17 17l2.1 2.1M4.9 19.1 7 17M17 7l2.1-2.1" />
        </>
      ),
      collapse: <path d="M11 17l-5-5 5-5M18 17l-5-5 5-5" />,
      expand: <path d="M13 17l5-5-5-5M6 17l5-5-5-5" />,
    }[name],
    "h-[18px] w-[18px]",
  );

/** Visible label of an item kind. */
export const KIND_LABEL: Record<ItemKind, string> = { pr: "PR", issue: "Issue", todo: "TODO" };

/** Spinner (static with reduced motion). */
export const Spinner = () => (
  <span
    aria-hidden
    data-spinner
    className="inline-block h-[11px] w-[11px] flex-none animate-spin rounded-full border-2 border-accent border-t-transparent motion-reduce:animate-none motion-reduce:opacity-60"
  />
);
