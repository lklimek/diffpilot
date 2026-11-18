import { expect, it } from "vitest";
import type { Annotation, Step, Tour } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { remarkTarget, summariseRemarks } from "./remarks";

const tour = seedFor("default", "tour") as Tour;
const steps = tour.flows.flatMap((f) => f.steps);
const rated = (band: "high" | "medium" | "low" | null, n: number, id = "s"): Step => {
  const base = steps[0]!;
  return {
    ...base,
    id,
    llm: {
      ...base.llm,
      assessment: band === null ? null : { ...base.llm.assessment!, risk: band },
      annotations: Array.from({ length: n }, (_, i) => ({
        ...base.llm.annotations[0]!,
        id: `${id}.a${i}`,
        range: { start_line: 10 + i, end_line: 10 + i },
      })),
    },
  } as Step;
};

it("a step: its LLM rating and its remark count, as two separate facts", () => {
  expect(summariseRemarks([rated("high", 5)], new Set())).toEqual({
    rating: "high",
    rated: 1,
    remarks: 5,
    handled: 0,
    flags: { red: 0, yellow: 0, unknown: 0 },
    ratings: [{ rating: "high", steps: 1 }],
  });
});

it("remarks on an unrated step still count; a rating without remarks stands alone; neither gives nothing", () => {
  expect(summariseRemarks([rated(null, 2)], new Set())).toMatchObject({ rating: null, remarks: 2 });
  expect(summariseRemarks([rated("low", 0)], new Set())).toMatchObject({ rating: "low", remarks: 0 });
  expect(summariseRemarks([rated(null, 0)], new Set())).toBeNull();
});

it("a flow: the highest step rating with the number of steps at it, all remarks, the ratings per step", () => {
  const flow = [rated("low", 3, "a"), rated("high", 1, "b"), rated("high", 4, "c"), rated(null, 2, "d")];
  expect(summariseRemarks(flow, new Set(["a.a0", "d.a1"]))).toEqual({
    rating: "high",
    rated: 2,
    remarks: 10,
    handled: 2,
    flags: { red: 0, yellow: 0, unknown: 0 },
    ratings: [
      { rating: "high", steps: 2 },
      { rating: "low", steps: 1 },
      { rating: null, steps: 1 },
    ],
  });
});

it("leads to the first remark on a step at the highest rating, one without feedback first", () => {
  const flow = [rated("low", 1, "a"), rated("high", 0, "b"), rated("high", 2, "c")];
  expect(remarkTarget(flow, new Set())).toMatchObject({ step: { id: "c" }, annotation: { id: "c.a0" } });
  expect(remarkTarget(flow, new Set(["c.a0"]))).toMatchObject({ annotation: { id: "c.a1" } });
  // No remark at the highest rating: the first remark anywhere; no remark at all: the rated step itself.
  expect(remarkTarget([rated("high", 0, "b"), rated("low", 1, "a")], new Set())).toMatchObject({
    step: { id: "a" },
    annotation: { id: "a.a0" },
  });
  expect(remarkTarget([rated("medium", 0, "b")], new Set())).toEqual({
    step: expect.objectContaining({ id: "b" }),
    annotation: null,
  });
  expect(remarkTarget([rated(null, 0)], new Set())).toBeUndefined();
});

it("counts the flags among the remarks by level, a flag without a level as unknown", () => {
  const flagged = rated("high", 3, "f");
  const [a, b, c] = flagged.llm.annotations as Annotation[];
  Object.assign(a!, { type: "flag", flag: { level: "red", scenario: ["x", "y"], not_checked: "z" } });
  Object.assign(b!, { type: "flag" });
  Object.assign(c!, { type: "flag", flag: { level: "yellow", scenario: ["x", "y"], not_checked: "z" } });
  expect(summariseRemarks([flagged], new Set())?.flags).toEqual({ red: 1, yellow: 1, unknown: 1 });
});
