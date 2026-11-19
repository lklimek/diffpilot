import type { FileViewedState } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";

/** How a GitHub Viewed mark is drawn: glyph, colour, short text, and what a screen reader hears in the tree. */
export const VIEWED_LOOK: Record<
  FileViewedState | "unread",
  { glyph: string; text: string; spoken: string; className: string }
> = {
  viewed: { glyph: "☑", text: "viewed", spoken: "viewed on GitHub", className: "text-accent" },
  dismissed: {
    glyph: "↻",
    text: "changed since viewed",
    spoken: "changed since viewed",
    className: "font-bold text-med",
  },
  unviewed: { glyph: "☐", text: "not viewed", spoken: "not viewed", className: "text-muted" },
  unread: { glyph: "?", text: "Viewed not read", spoken: "Viewed not read", className: "text-muted" },
};

/**
 * The file's GitHub Viewed mark in a Flows step header (read-only); null when GitHub's state was not read. Set
 * in the Files view or on GitHub; separate from step ticks.
 */
export function ViewedChip({ state, path }: { state: FileViewedState | null; path: string }) {
  const look = VIEWED_LOOK[state ?? "unread"];
  return (
    <Tooltip
      tip={`Your GitHub Viewed mark for ${path}. Set it in the Files view (v) or on GitHub; separate from step ticks.`}
    >
      <span
        tabIndex={0}
        data-viewed={state ?? "unread"}
        className="cursor-help rounded border border-border px-1.5 text-[11px] leading-[18px] font-semibold whitespace-nowrap"
      >
        <span className={look.className}>{look.glyph}</span> {look.text}
      </span>
    </Tooltip>
  );
}
