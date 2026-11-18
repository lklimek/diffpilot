// What Home offers after resolving a PR URL (brief §4.1).
import type { AnalysisSummary, ResolvePrResponse } from "../../api/generated/types";

/** Home's answer to a resolved PR. */
export type HomeOutcome =
  | { kind: "analyze" }
  | { kind: "resume"; analysis: AnalysisSummary }
  | { kind: "in_progress"; analysis: AnalysisSummary }
  | { kind: "failed"; analysis: AnalysisSummary; message: string }
  | { kind: "head_moved"; analysis: AnalysisSummary };

/** No analysis (or a cancelled one): analyze; head moved: offer Re-analyze; else resume the analysis. */
export function homeOutcome(resolved: ResolvePrResponse): HomeOutcome {
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

/** "Resume review" returns to a review this user started; "Open review" opens one not started yet. */
export const reviewLabel = (analysis: { has_my_state?: boolean }) =>
  analysis.has_my_state === true ? "Resume review" : "Open review";
