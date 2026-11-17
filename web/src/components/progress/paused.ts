import type { AnalysisState } from "../../api/generated/types";
import { clockText } from "../format/time";

/** "Paused until HH:MM — k of n"; without a count while planning, without a time when it is not known. */
export function pausedText(state: Extract<AnalysisState, { state: "paused" }>): string {
  const until = state.resumes_at
    ? `Paused until ${clockText(state.resumes_at)}`
    : "Paused; the resume time is not known";
  return state.total === null ? until : `${until} — ${state.done} of ${state.total}`;
}

/** Why an analysis pauses (the server's sentence for a usage limit). */
export const PAUSE_REASON = "The LLM provider's usage limit was reached.";
