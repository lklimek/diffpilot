import type { Evidence, LineRange } from "../../api/generated/types";

/** `file:line` of an evidence reference; base-side lines are marked. */
export const evidenceText = (e: Evidence) => `${e.file}:${e.line}${e.side === "base" ? " (base)" : ""}`;

/** `a–b`, or `a` for a one-line range. */
export const rangeText = (r: LineRange) =>
  r.start_line === r.end_line ? `${r.start_line}` : `${r.start_line}–${r.end_line}`;
