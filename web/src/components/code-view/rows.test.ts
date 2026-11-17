import { describe, expect, it } from "vitest";
import type { Annotation, LinesResponse } from "../../api/generated/types";
import { buildSegments, type Segment } from "./rows";

function lines(from: number, to: number, extra: Partial<LinesResponse> = {}): LinesResponse {
  return {
    path: "src/a.rs",
    side: "head",
    from,
    to,
    total_lines: to,
    lines: Array.from({ length: to - from + 1 }, (_, i) => `line ${from + i}`),
    changed: [],
    deleted: [],
    moved: [],
    comments: { scope: "not_in_diff" },
    ...extra,
  };
}

function annotation(id: string, start: number, end: number): Annotation {
  return {
    id,
    type: "note",
    basis: "fact",
    range: { start_line: start, end_line: end },
    was: "w",
    is: "i",
    text: "t",
    evidence: [],
  };
}

const shape = (segments: Segment[]) =>
  segments.map((s) =>
    s.kind === "deleted"
      ? `deleted@${s.block.after_line}`
      : `${s.kind}:${s.rows[0]!.line}-${s.rows.at(-1)!.line}${
          s.kind === "highlight" ? `[${s.annotations.map((a) => a.id).join(",")}]` : ""
        }`,
  );

describe("buildSegments", () => {
  it("groups overlapping annotations into one highlight and keeps adjacent disjoint ones apart", () => {
    const segments = buildSegments(lines(1, 10), [
      annotation("a", 2, 4),
      annotation("b", 3, 5),
      annotation("c", 6, 7),
    ]);
    expect(shape(segments)).toEqual(["rows:1-1", "highlight:2-5[a,b]", "highlight:6-7[c]", "rows:8-10"]);
  });

  it("ignores annotation lines outside the displayed range", () => {
    expect(shape(buildSegments(lines(5, 8), [annotation("a", 1, 6), annotation("z", 20, 30)]))).toEqual([
      "highlight:5-6[a]",
      "rows:7-8",
    ]);
  });

  it("places deleted blocks after their anchor line, before the first line for 0, and splits a highlight there", () => {
    const deleted = [0, 2].map((after_line) => ({ after_line, base_start: 1, lines: ["x"], total_lines: 1 }));
    expect(shape(buildSegments(lines(1, 4, { deleted }), [annotation("a", 2, 3)]))).toEqual([
      "deleted@0",
      "rows:1-1",
      "highlight:2-2[a]",
      "deleted@2",
      "highlight:3-3[a]",
      "rows:4-4",
    ]);
  });

  it("derives the per-line facts: changed, moved, differing from the moved original", () => {
    const segments = buildSegments(
      lines(1, 4, { changed: [2], moved: [{ range: { start_line: 2, end_line: 3 }, differing: [3] }] }),
      [],
    );
    const rows = segments.flatMap((s) => (s.kind === "deleted" ? [] : s.rows));
    expect(rows.map((r) => [r.line, r.changed, r.moved, r.differs])).toEqual([
      [1, false, false, false],
      [2, true, true, false],
      [3, false, true, true],
      [4, false, false, false],
    ]);
    expect(rows.map((r) => r.index)).toEqual([0, 1, 2, 3]);
  });
});
