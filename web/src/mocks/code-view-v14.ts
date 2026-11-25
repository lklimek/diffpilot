import type { LinesResponse, Tour } from "../api/generated/types";
import type { StepFactsWithLeadingContext } from "../api/provisional/stepFacts";

export function codeViewTour(seed: Tour): Tour {
  const tour = structuredClone(seed);
  const step = tour.flows.flatMap((f) => f.steps).find((s) => s.id === "f1.2");
  if (!step) return tour;
  const facts: StepFactsWithLeadingContext = {
    ...step.facts,
    leading_context: { start_line: 85, end_line: 87 },
  };
  step.facts = facts;
  const remark = step.llm.annotations[0];
  if (remark) remark.is += " See `rpc/client.rs:12-14`.";
  return tour;
}

export function codeViewLines(lines: LinesResponse): LinesResponse {
  if (lines.path !== "src/rpc/client.rs" || lines.side !== "head" || lines.from !== 85) return lines;
  return {
    ...lines,
    lines: ["/// Builds an RPC client.", "/// Each request uses the configured retry policy.", "#[must_use]"],
  };
}
