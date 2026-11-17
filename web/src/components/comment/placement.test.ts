import { describe, expect, it } from "vitest";
import type { CommentTarget, LinesResponse } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { commentPlacement, isChangedLine } from "./placement";

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
const withDeleted: LinesResponse = {
  ...inDiff,
  deleted: [
    { after_line: 90, base_start: 10, lines: ["a", "b"], total_lines: 2 },
    { after_line: 101, base_start: 30, lines: ["c", "d"], total_lines: 2 },
  ],
};
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

  it("gives a line thread to removed lines inside one deletion block", () => {
    expect(commentPlacement(withDeleted, at(11, { side: "base" }))).toBe("line");
    expect(commentPlacement(withDeleted, at(11, { side: "base", start_line: 10 }))).toBe("line");
  });

  it("leaves a removed range spanning two deletion blocks to the server, never previewing a line thread", () => {
    expect(commentPlacement(withDeleted, at(31, { side: "base", start_line: 10 }))).toBe("on_post");
  });

  it("leaves a removed line outside every deletion block of the response to the server", () => {
    expect(commentPlacement(withDeleted, at(20, { side: "base" }))).toBe("on_post");
  });

  it("does not comment on a file outside the PR diff", () => {
    expect(commentPlacement(lines({ scope: "not_in_diff" }), at(10))).toBe("not_in_diff");
  });

  it("matches the seeded lines of src/rpc/client.rs", () => {
    const seeded = seedFor("default", "lines/head/src/rpc/client.rs") as LinesResponse;
    expect(commentPlacement(seeded, at(96))).toBe("line");
  });
});

describe("isChangedLine", () => {
  const shown: LinesResponse = {
    ...inDiff,
    changed: [89, 96],
    deleted: [{ after_line: 90, base_start: 101, lines: ["a", "b", "c"], total_lines: 3 }],
  };
  const at = (side: "head" | "base", line: number): CommentTarget => ({
    path: "src/rpc/client.rs",
    side,
    start_line: null,
    line,
  });

  it("is true for a changed line on its side and for a removed line at a deletion marker", () => {
    expect(isChangedLine(shown, at("head", 96))).toBe(true);
    expect(isChangedLine(shown, at("base", 103))).toBe(true);
  });

  it("is false for an unchanged line, a line outside the deletion, and another file", () => {
    expect(isChangedLine(shown, at("head", 88))).toBe(false);
    expect(isChangedLine(shown, at("base", 104))).toBe(false);
    expect(isChangedLine(shown, { ...at("head", 96), path: "src/other.rs" })).toBe(false);
  });
});
