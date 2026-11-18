import { expect, it } from "vitest";
import type { Step, Tour } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { firstRemark, remarkCounts } from "./remarks";

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

it("counts each step's remarks under the step's LLM risk band, highest first; no zero entries", () => {
  const counts = remarkCounts([rated("low", 3, "a"), rated("high", 1, "b"), rated("low", 0, "c")], new Set());
  expect(counts).toEqual([
    { band: "high", count: 1, handled: 0 },
    { band: "low", count: 3, handled: 0 },
  ]);
});

it("leaves remarks of an unrated step uncounted (no severity on the wire)", () => {
  expect(remarkCounts([rated(null, 2)], new Set())).toEqual([]);
});

it("counts remarks with the user's feedback as handled", () => {
  expect(remarkCounts([rated("medium", 2, "m")], new Set(["m.a0"]))).toEqual([
    { band: "medium", count: 2, handled: 1 },
  ]);
});

it("finds the first remark of a band in order, preferring one not handled yet", () => {
  const list = [rated("low", 1, "a"), rated("high", 2, "b")];
  expect(firstRemark(list, "high", new Set())).toMatchObject({
    step: { id: "b" },
    annotation: { id: "b.a0" },
  });
  expect(firstRemark(list, "high", new Set(["b.a0"]))).toMatchObject({ annotation: { id: "b.a1" } });
  expect(firstRemark(list, "medium", new Set())).toBeUndefined();
});
