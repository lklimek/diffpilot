import type { AnalysisState } from "../../api/generated/types";

/**
 * A running analysis with no flow job queued or running: nothing goes on until a flow is requested.
 * Inferred from the absent `stage` until the server's `idle` field replaces it here.
 */
export const isIdle = (state: AnalysisState): boolean => state.state === "running" && !state.stage;
