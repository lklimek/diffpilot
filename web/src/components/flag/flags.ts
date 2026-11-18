import type { Annotation, FlagLevel } from "../../api/generated/types";

/**
 * How each flag level is shown: a word with the ⚑ glyph and token colours, so the colour is never the only
 * signal. `unknown` is a flag stored before flags had levels (a former question to check).
 */
export const FLAG_LOOK: Record<
  FlagLevel,
  { name: string; label: string; tip: string; text: string; bg: string; border: string; line: string }
> = {
  red: {
    name: "Red flag",
    label: "⚑ Red flag",
    tip: "Red flag: the LLM sees a likely defect or damage if this scenario is reachable. LLM statement, not checked.",
    text: "text-high",
    bg: "bg-high-bg",
    border: "border-high-bd",
    line: "bg-[linear-gradient(var(--color-high-bd),var(--color-high-bd))]",
  },
  yellow: {
    name: "Yellow flag",
    label: "⚑ Yellow flag",
    tip: "Yellow flag: the LLM thinks this is worth a look, plausible but weaker or of lower cost. LLM statement, not checked.",
    text: "text-med",
    bg: "bg-med-bg",
    border: "border-med-bd",
    line: "bg-[linear-gradient(var(--color-med-bd),var(--color-med-bd))]",
  },
  unknown: {
    name: "Flag",
    label: "⚑ Flag",
    tip: "Flag without a level: this analysis was stored before flags had levels, so it has no scenario. LLM statement, not checked.",
    text: "text-llm",
    bg: "bg-llm-bg",
    border: "border-mark",
    line: "bg-[linear-gradient(var(--color-mark),var(--color-mark))]",
  },
};

/** Flag levels, strongest first. */
export const FLAG_ORDER: readonly FlagLevel[] = ["red", "yellow", "unknown"];

/** The level of a flag annotation (`unknown` when the server sent none); null for other annotation types. */
export const flagLevel = (annotation: Annotation): FlagLevel | null =>
  annotation.type === "flag" ? (annotation.flag?.level ?? "unknown") : null;

/** The strongest level among `annotations`; null without flags. */
export function strongestFlag(annotations: readonly Annotation[]): FlagLevel | null {
  const levels = new Set(annotations.map(flagLevel));
  return FLAG_ORDER.find((level) => levels.has(level)) ?? null;
}

/** Flags first by level (strongest first), then by line; other annotations are left out. */
export const sortedFlags = (annotations: readonly Annotation[]): Annotation[] =>
  annotations
    .filter((a) => a.type === "flag")
    .sort(
      (a, b) =>
        FLAG_ORDER.indexOf(flagLevel(a)!) - FLAG_ORDER.indexOf(flagLevel(b)!) ||
        a.range.start_line - b.range.start_line,
    );

/** Number of flags per level. */
export type FlagCounts = Record<FlagLevel, number>;

/** Flags of `annotations` counted by level. */
export function countFlags(annotations: readonly Annotation[]): FlagCounts {
  const counts: FlagCounts = { red: 0, yellow: 0, unknown: 0 };
  for (const a of annotations) {
    const level = flagLevel(a);
    if (level) counts[level] += 1;
  }
  return counts;
}

const WORD: Record<FlagLevel, string> = {
  red: "red flag",
  yellow: "yellow flag",
  unknown: "flag without a level",
};
const WORDS: Record<FlagLevel, string> = {
  red: "red flags",
  yellow: "yellow flags",
  unknown: "flags without a level",
};

/** "1 red flag, 2 yellow flags"; null without flags (never an all-clear). */
export function flagCountText(counts: FlagCounts): string | null {
  const parts = FLAG_ORDER.filter((level) => counts[level] > 0).map(
    (level) => `${counts[level]} ${counts[level] === 1 ? WORD[level] : WORDS[level]}`,
  );
  return parts.length > 0 ? parts.join(", ") : null;
}
