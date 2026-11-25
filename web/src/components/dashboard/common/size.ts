import type { ChangeSizeMark, SizeMark } from "../../../api/generated/types";

/**
 * The size marks in order with the lowest changed-line count of each (server `score::SIZE_MARKS`, set by the owner,
 * 2026-11-18). The list supplies the size rule text.
 */
export const SIZE_CLASSES: readonly { size: SizeMark; from: number }[] = [
  { size: "tiny", from: 0 },
  { size: "small", from: 150 },
  { size: "medium", from: 500 },
  { size: "large", from: 3000 },
  { size: "huge", from: 10_000 },
];

/** Every size class, in order. */
export const ALL_SIZES: SizeMark[] = SIZE_CLASSES.map((c) => c.size);

/** `10 000`; smaller counts without a separator. */
const lines = (n: number) => (n >= 10_000 ? n.toLocaleString("en-US").replaceAll(",", " ") : String(n));

/** The range of one class: `< 500`, `500–2999`, `≥ 10 000`. */
export function sizeRange(index: number): string {
  const from = SIZE_CLASSES[index]!.from;
  const next = SIZE_CLASSES[index + 1]?.from;
  if (index === 0 && next !== undefined) return `< ${lines(next)}`;
  return next === undefined ? `≥ ${lines(from)}` : `${lines(from)}–${lines(next - 1)}`;
}

/** `small < 500, medium 500–2999, large 3000–9999, huge ≥ 10 000 changed lines`. */
export const SIZE_RULE = `${SIZE_CLASSES.map((c, i) => `${c.size} ${sizeRange(i)}`).join(", ")} changed lines`;

/** Changed lines of a mark. */
export const changedLines = (mark: ChangeSizeMark) => mark.additions + mark.deletions;
