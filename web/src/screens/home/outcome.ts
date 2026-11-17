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

/** Analyze without asking: nothing to resume and nothing to warn about (the App is installed). */
export const startsAtOnce = (resolved: ResolvePrResponse) =>
  homeOutcome(resolved).kind === "analyze" && resolved.commenting_available;

/** The review of a finished analysis, else its progress screen. */
export const analysisHref = (analysis: AnalysisSummary) =>
  analysis.state.state === "finished" ? `/analyses/${analysis.id}` : `/analyses/${analysis.id}/progress`;
