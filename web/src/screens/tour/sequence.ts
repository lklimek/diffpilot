// The Tour sequence (brief §4.3): PR context, then per flow its introduction and its steps.
import type { BlockRef, Flow, RangeQuery, Step, StepId, Tour } from "../../api/generated/types";

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

/**
 * The introduction of the next (`1`) or previous (`-1`) flow, for `n` / `p`: from PR context `n` opens flow 1;
 * outside the flows (quiet rows, pending review) `n` opens the first flow and `p` the last.
 */
export function flowJump(tour: Tour, current: BlockRef, direction: 1 | -1): BlockRef | undefined {
  if (current.block === "pr_context" && direction < 0) return undefined;
  const flowId =
    current.block === "flow_intro"
      ? current.flow_id
      : current.block === "step"
        ? findStep(tour, current.step_id)?.flow.id
        : undefined;
  const at = tour.flows.findIndex((f) => f.id === flowId);
  const index =
    at < 0
      ? direction > 0
        ? 0
        : tour.flows.length - 1
      : Math.max(0, Math.min(tour.flows.length - 1, at + direction));
  const flow = tour.flows[index];
  return flow && { block: "flow_intro", flow_id: flow.id };
}

/** The lines of a step's range. */
export const stepQuery = (step: Step): RangeQuery => ({
  path: step.facts.file,
  side: step.facts.side,
  from: step.facts.range.start_line,
  to: step.facts.range.end_line,
});
