import type { SizeWarning as Warning } from "../../api/generated/types";

const grouped = (value: number) => new Intl.NumberFormat("en").format(value).replaceAll(",", " ");

/** A pre-start heads-up; the Analyze button remains available. */
export function SizeWarning({ warning }: { warning: Warning }) {
  const minutes = warning.estimate && Math.ceil(warning.estimate.remaining_seconds / 60);
  const duration =
    minutes === null ? null : minutes < 60 ? `${minutes} min` : `${(minutes / 60).toFixed(1)} h`;
  return (
    <p role="note" className="mt-2 text-sm text-muted">
      Large PR: {grouped(warning.changed_lines)} changed lines in {grouped(warning.changed_files)} files.{" "}
      {warning.estimate ? (
        <>
          {warning.estimate.remaining_seconds >= 10800
            ? "Based on recent runs here this may take several hours and many LLM calls. "
            : "This may require many LLM calls. "}
          About {duration} ({warning.estimate.rough ? "rough estimate" : "estimate"} from{" "}
          {warning.estimate.basis === "lines_only" ? "changed lines in recent runs" : "recent runs here"}).
        </>
      ) : (
        "This may require many LLM calls. No time estimate yet."
      )}
    </p>
  );
}
