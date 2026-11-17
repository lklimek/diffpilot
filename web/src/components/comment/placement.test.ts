import { describe, expect, it } from "vitest";
import type { CommentTarget, LinesResponse } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { commentPlacement } from "./placement";

const lines = (comments: LinesResponse["comments"]): LinesResponse => ({
  path: "src/rpc/client.rs",
  side: "head",
  from: 80,
  to: 120,
  total_lines: 300,
  lines: [],
  changed: [],
  deleted: [],
  moved: [],
  comments,
});
const inDiff = lines({
  scope: "in_diff",
  line_thread_ranges: [
    { start_line: 88, end_line: 99 },
    { start_line: 104, end_line: 106 },
  ],
});
const at = (line: number, over: Partial<CommentTarget> = {}): CommentTarget => ({
  path: "src/rpc/client.rs",
  side: "head",
  start_line: null,
  line,
  ...over,
});

describe("commentPlacement (ADR-006)", () => {
  it("gives a line thread to a line inside a diff hunk", () => {
    expect(commentPlacement(inDiff, at(96))).toBe("line");
    expect(commentPlacement(inDiff, at(99, { start_line: 88 }))).toBe("line");
  });

  it("posts a line of a PR file outside the hunks as a file comment", () => {
    expect(commentPlacement(inDiff, at(101))).toBe("file");
  });

  it("posts a range spanning two hunks as a file comment", () => {
    expect(commentPlacement(inDiff, at(105, { start_line: 98 }))).toBe("file");
  });

  it("gives a line thread to a removed line shown at a deletion marker", () => {
    expect(commentPlacement(inDiff, at(55, { side: "base" }))).toBe("line");
  });

  it("does not comment on a file outside the PR diff", () => {
    expect(commentPlacement(lines({ scope: "not_in_diff" }), at(10))).toBe("not_in_diff");
  });

  it("matches the seeded lines of src/rpc/client.rs", () => {
    const seeded = seedFor("default", "lines/head/src/rpc/client.rs") as LinesResponse;
    expect(commentPlacement(seeded, at(96))).toBe("line");
  });
});
