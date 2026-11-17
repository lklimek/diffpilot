import { describe, expect, it } from "vitest";
import type { Tour } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { blockTitle, buildSequence, findStep, flowJump, sameBlock } from "./sequence";

const tour = seedFor("default", "tour") as Tour;

describe("tour sequence (brief §4.3)", () => {
  const sequence = buildSequence(tour);

  it("starts at the PR context, then each flow introduction followed by its steps", () => {
    expect(sequence.slice(0, 4)).toEqual([
      { block: "pr_context" },
      { block: "flow_intro", flow_id: "f1" },
      { block: "step", step_id: "f1.1" },
      { block: "step", step_id: "f1.2" },
    ]);
    const f2 = sequence.findIndex((b) => sameBlock(b, { block: "flow_intro", flow_id: "f2" }));
    expect(sequence[f2 - 1]).toEqual({ block: "step", step_id: "f1.10" });
  });

  it("has one block per introduction and step", () => {
    const steps = tour.flows.reduce((n, f) => n + f.steps.length, 0);
    expect(sequence).toHaveLength(1 + tour.flows.length + steps);
  });

  it("titles blocks for Previous / Next", () => {
    expect(blockTitle(tour, { block: "pr_context" })).toBe("PR context");
    expect(blockTitle(tour, { block: "flow_intro", flow_id: "f1" })).toBe(
      "Flow 1: Send an RPC request with retry",
    );
    expect(blockTitle(tour, { block: "step", step_id: "f1.5" })).toBe("Step 5: Retry allowed?");
  });

  it("finds a step with its flow", () => {
    expect(findStep(tour, "f2.1")).toMatchObject({ flow: { id: "f2" }, step: { id: "f2.1" } });
    expect(findStep(tour, "f9.9")).toBeUndefined();
  });

  it("compares block references by value", () => {
    expect(sameBlock({ block: "step", step_id: "f1.1" }, { block: "step", step_id: "f1.1" })).toBe(true);
    expect(sameBlock({ block: "step", step_id: "f1.1" }, { block: "step", step_id: "f1.2" })).toBe(false);
    expect(sameBlock({ block: "pr_context" }, { block: "flow_intro", flow_id: "f1" })).toBe(false);
  });

  it("n / p jump to the next or previous flow introduction, clamped at the ends", () => {
    const intro = (flow_id: string) => ({ block: "flow_intro", flow_id }) as const;
    expect(flowJump(tour, { block: "pr_context" }, 1)).toEqual(intro("f1"));
    expect(flowJump(tour, { block: "pr_context" }, -1)).toBeUndefined();
    expect(flowJump(tour, { block: "step", step_id: "f2.1" }, 1)).toEqual(intro("f3"));
    expect(flowJump(tour, { block: "step", step_id: "f2.1" }, -1)).toEqual(intro("f1"));
    expect(flowJump(tour, intro("f1"), -1)).toEqual(intro("f1"));
    expect(flowJump(tour, intro("f5"), 1)).toEqual(intro("f5"));
    expect(flowJump(tour, { block: "pending_review" }, -1)).toEqual(intro("f5"));
    expect(flowJump(tour, { block: "quiet", row: { row: "moves" } }, 1)).toEqual(intro("f1"));
  });
});
