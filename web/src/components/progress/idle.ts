import type { AnalysisState } from "../../api/generated/types";

/**
 * A running analysis with nothing admitted for analysis: nothing goes on until a flow is requested.
 * The server's `idle` (not yet in the generated types) wins; without it, an absent `stage` is the sign.
 */
export function isIdle(state: AnalysisState): boolean {
  if (state.state !== "running") return false;
  const { idle } = state as { idle?: boolean };
  return idle ?? !state.stage;
}
