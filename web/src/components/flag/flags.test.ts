import { expect, it } from "vitest";
import type { Annotation, FlagLevel } from "../../api/generated/types";
import { countFlags, flagCountText, flagLevel, sortedFlags, strongestFlag } from "./flags";

const remark = (id: string, line: number, level?: FlagLevel | "none"): Annotation => ({
  id,
  type: level === undefined ? "note" : "flag",
  basis: "inference",
  range: { start_line: line, end_line: line },
  was: "",
  is: "",
  text: "Potential race:",
  evidence: [],
  ...(level && level !== "none" ? { flag: { level, scenario: ["a", "b"], not_checked: "c" } } : {}),
});

it("reads a flag's level; a flag the server sent without one is unknown; other types have none", () => {
  expect(flagLevel(remark("a", 1, "red"))).toBe("red");
  expect(flagLevel(remark("b", 1, "none"))).toBe("unknown");
  expect(flagLevel(remark("c", 1))).toBeNull();
});

it("orders flags red, yellow, unknown, then by line, leaving other remarks out", () => {
  const list = [
    remark("n", 1),
    remark("u", 2, "unknown"),
    remark("y2", 9, "yellow"),
    remark("r", 30, "red"),
    remark("y1", 3, "yellow"),
  ];
  expect(sortedFlags(list).map((a) => a.id)).toEqual(["r", "y1", "y2", "u"]);
  expect(strongestFlag(list)).toBe("red");
  expect(strongestFlag([remark("n", 1)])).toBeNull();
});

it("counts flags by level in words, and says nothing without flags (never an all-clear)", () => {
  const counts = countFlags([remark("r", 1, "red"), remark("y", 2, "yellow"), remark("z", 3, "yellow")]);
  expect(flagCountText(counts)).toBe("1 red flag, 2 yellow flags");
  expect(flagCountText(countFlags([remark("u", 1, "unknown")]))).toBe("1 flag without a level");
  expect(flagCountText(countFlags([remark("n", 1)]))).toBeNull();
});
