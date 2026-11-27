// What the PR panel offers after resolving a PR URL (brief §4.1).
import type { AnalysisSummary, ResolvePrResponse } from "../../api/generated/types";

/** The panel’s answer to a resolved PR. */
export type ReviewOutcome =
  | { kind: "analyze" }
  | { kind: "resume"; analysis: AnalysisSummary }
  | { kind: "in_progress"; analysis: AnalysisSummary }
  | { kind: "failed"; analysis: AnalysisSummary; message: string }
  | { kind: "head_moved"; analysis: AnalysisSummary };

/** No analysis (or a cancelled one): offer Start analysis; head moved: offer a fresh one; else resume the analysis. */
export function reviewOutcome(resolved: ResolvePrResponse): ReviewOutcome {
  const { analysis } = resolved;
  if (!analysis || analysis.state.state === "cancelled") return { kind: "analyze" };
  if (resolved.head_moved) return { kind: "head_moved", analysis };
  switch (analysis.state.state) {
    case "finished":
      return { kind: "resume", analysis };
    case "failed":
      return { kind: "failed", analysis, message: analysis.state.message };
    default:
      return { kind: "in_progress", analysis };
  }
}

/** The review of a finished analysis, else its progress screen. */
export const analysisHref = (analysis: AnalysisSummary) =>
  analysis.review_ready || analysis.state.state === "finished"
    ? `/analyses/${analysis.id}`
    : `/analyses/${analysis.id}/progress`;

/** Tooltip of the "Open" link of an analysis: it only navigates; ticks and answers are kept once the user started. */
export const openTip = (analysis: { has_my_state?: boolean }) =>
  analysis.has_my_state === true
    ? "Opens your review; ticks and answers are kept. Starts nothing."
    : "Opens the review. Starts nothing.";
