// Flows view: one step's remarks over a replacement keep its replacing lines in the "Changed: N lines replaced"
// block (K5); only another step's remarks may split it (UX-017).
import { describe, expect, it } from "vitest";
import type { Annotation, LinesResponse } from "../../api/generated/types";
import { buildSegments, type Segment } from "./rows";

const view: LinesResponse = {
  path: "src/a.rs",
  side: "head",
  from: 1,
  to: 5,
  total_lines: 5,
  lines: ["l1", "l2", "l3", "l4", "l5"],
  changed: [1, 2, 3, 4, 5],
  deleted: [
    {
      after_line: 0,
      base_start: 1,
      lines: ["old"],
      total_lines: 1,
      replacement: { start_line: 1, end_line: 5 },
    },
  ],
  moved: [],
  comments: { scope: "not_in_diff" },
} as LinesResponse;

const note = (id: string, start: number, end: number): Annotation => ({
  id,
  type: "note",
  basis: "fact",
  range: { start_line: start, end_line: end },
  was: "w",
  is: "i",
  text: "t",
  evidence: [],
});

describe("buildSegments in the Flows view (one step)", () => {
  it("a second remark of the same step inside a replacement keeps every replacing line paired", () => {
    const segments = buildSegments(view, [note("a", 1, 5), note("b", 3, 3)]);
    const block = segments[0] as Extract<Segment, { kind: "deleted" }>;
    expect(block.paired.map((r) => r.line)).toEqual([1, 2, 3, 4, 5]);
  });
});
