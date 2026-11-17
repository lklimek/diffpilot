import type { Flow, StepId } from "../../api/generated/types";

/** Number of the flow's steps the user ticked. */
export const reviewedSteps = (flow: Flow, ticked: readonly StepId[]): number =>
  flow.steps.filter((s) => ticked.includes(s.id)).length;

/** A flow is reviewed when all its steps are ticked (brief §2, derived, not stored). */
export const isFlowReviewed = (flow: Flow, ticked: readonly StepId[]): boolean =>
  flow.steps.length > 0 && reviewedSteps(flow, ticked) === flow.steps.length;
