import type { AnalysisEstimate, SizeWarning as Warning } from "../../api/generated/types";

/** `27 081`: thousands grouped by a space. */
export const grouped = (value: number) => new Intl.NumberFormat("en").format(value).replaceAll(",", " ");

/** `Planning: about 25 min (estimate from recent runs here).`: the estimate covers plan and PR context only. */
export function estimateText(estimate: AnalysisEstimate): string {
  const minutes = Math.ceil(estimate.remaining_seconds / 60);
  const duration = minutes < 60 ? `${minutes} min` : `${(minutes / 60).toFixed(1)} h`;
  const basis = estimate.basis === "lines_only" ? "changed lines in recent runs" : "recent runs here";
  return `Planning: about ${duration} (${estimate.rough ? "rough estimate" : "estimate"} from ${basis}).`;
}

/** A pre-start heads-up above "Start analysis anyway"; it never prevents starting. */
export function SizeWarning({ warning }: { warning: Warning }) {
  return (
    <p role="note" className="mt-2 text-sm text-muted">
      Large PR: {grouped(warning.changed_lines)} changed lines in {grouped(warning.changed_files)} files.{" "}
      {warning.estimate ? (
        <>
          {warning.estimate.remaining_seconds >= 10800
            ? "Based on recent runs here planning alone may take several hours and many LLM calls. "
            : "This may require many LLM calls. "}
          {estimateText(warning.estimate)}
        </>
      ) : (
        "This may require many LLM calls. No time estimate yet."
      )}
    </p>
  );
}
