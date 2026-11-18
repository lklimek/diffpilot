import type { FlagLevel, RiskBand } from "../../api/generated/types";
import { FLAG_LOOK } from "../flag/flags";

/**
 * How a changed line is drawn, in the code view and the classic diff alike (deterministic, from the diff):
 * green fill with `+` for added, red fill with `−` for removed. LLM remarks never fill lines.
 */
export const CHANGE_LOOK = {
  added: { sign: "+", text: "added", fill: "bg-ad-bg", signLook: "text-add" },
  removed: { sign: "−", text: "removed", fill: "bg-rm-bg", signLook: "text-high" },
} as const;

/** What a screen reader hears for a changed line's sign, in the code view and the classic diff alike. */
export const changeLineText = (kind: keyof typeof CHANGE_LOOK) => `${CHANGE_LOOK[kind].text} line`;

// A remark is a bar in the reserved 5 px column, never a fill: only changed lines look changed. The bar pattern
// is the colour-independent signal; an unrated remark gets a thin neutral line of its own, so it never reads as LOW.
/** The bar of an LLM remark range by the step's risk band (the LLM's rating, not checked). */
export const REMARK_LOOK: Record<
  RiskBand | "none",
  { label: string | null; pattern: string; bar: string; text: string }
> = {
  high: {
    label: "▲ HIGH",
    pattern: "solid",
    bar: "border-solid border-high-bd",
    text: "text-high",
  },
  medium: {
    label: "◆ MEDIUM",
    pattern: "dashed",
    bar: "border-dashed border-med-bd",
    text: "text-med",
  },
  low: {
    label: "● LOW",
    pattern: "dotted",
    bar: "border-dotted border-low-bd",
    text: "text-low",
  },
  none: {
    label: null,
    pattern: "thin",
    bar: "border-solid border-transparent bg-[linear-gradient(var(--color-mark),var(--color-mark))] bg-size-[2px_auto] bg-origin-border bg-no-repeat",
    text: "text-llm",
  },
};

const PATTERN = {
  solid: "border-solid",
  dashed: "border-dashed",
  dotted: "border-dotted",
  thin: "border-solid",
};

/** The bar of a range with an LLM flag: the flag's level colour, the step rating's pattern (solid when unrated). */
export const flagBar = (level: FlagLevel, band: RiskBand | null) =>
  `${PATTERN[REMARK_LOOK[band ?? "none"].pattern as keyof typeof PATTERN]} ${FLAG_LOOK[level].border}`;
