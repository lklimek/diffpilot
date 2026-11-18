import type { AnalysisState } from "../../api/generated/types";

/** A running analysis with no flow work admitted (`idle`, false when absent): nothing goes on until asked. */
export function isIdle(state: AnalysisState): boolean {
  return state.state === "running" && state.idle === true;
}
