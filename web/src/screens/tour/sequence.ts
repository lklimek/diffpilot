// The Tour sequence (brief §4.3): PR context, then per flow its introduction and its steps.
import type { BlockRef, Flow, Step, StepId, Tour } from "../../api/generated/types";

/** Blocks walked by Previous / Next and `j` / `k`, in order. */
export function buildSequence(tour: Tour): BlockRef[] {
  return [
    { block: "pr_context" },
    ...tour.flows.flatMap((flow): BlockRef[] => [
      { block: "flow_intro", flow_id: flow.id },
      ...flow.steps.map((step): BlockRef => ({ block: "step", step_id: step.id })),
    ]),
  ];
}

/** Whether two block references name the same block. */
export function sameBlock(a: BlockRef, b: BlockRef): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/** A step and the flow it belongs to. */
export function findStep(tour: Tour, stepId: StepId): { flow: Flow; step: Step } | undefined {
  for (const flow of tour.flows) {
    const step = flow.steps.find((s) => s.id === stepId);
    if (step) return { flow, step };
  }
  return undefined;
}

/** Short title of a block, e.g. "Flow 1: …" or "Step 5: …". */
export function blockTitle(tour: Tour, ref: BlockRef): string {
  switch (ref.block) {
    case "flow_intro": {
      const flow = tour.flows.find((f) => f.id === ref.flow_id);
      return flow ? `Flow ${flow.position}: ${flow.llm.title}` : "Flow";
    }
    case "step": {
      const found = findStep(tour, ref.step_id);
      return found ? `Step ${found.step.number}: ${found.step.llm.title}` : "Step";
    }
    case "pr_context":
      return "PR context";
    case "quiet":
      return "Quiet row";
    case "pending_review":
      return "Pending review";
  }
}
