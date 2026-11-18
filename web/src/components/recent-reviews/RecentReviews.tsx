import { useId } from "react";
import type { AnalysisStatus, MyAnalysis } from "../../api/generated/types";
import { agoText } from "../dashboard/common/format";
import { type Navigate, navigateOnClick } from "../dashboard/common/navigate";
import { dateTimeText } from "../format/time";
import { StatusMessage } from "../status/StatusMessage";

/** The stored status in the words of the progress screen. */
export const STATUS_TEXT: Record<AnalysisStatus, string> = {
  planning: "Planning flows",
  running: "Analysing flows",
  paused: "Paused",
  failed: "Analysis failed",
  finished: "Analysed",
  cancelled: "Analysis cancelled",
};

export interface RecentReviewsProps {
  /** The reader's own analyses, newest first; undefined while loading. */
  analyses: MyAnalysis[] | undefined;
  /** Why the list could not be loaded; null when it could. */
  error: string | null;
  /** In-app href of an analysis (built by the screen). */
  hrefOf: (analysis: MyAnalysis) => string;
  /** The time relative times are measured from. */
  now: Date;
  onNavigate?: Navigate;
}

/** Home's list of the reader's own analyses (Recent reviews). */
export function RecentReviews({ analyses, error, hrefOf, now, onNavigate }: RecentReviewsProps) {
  const titleId = useId();
  return (
    <section aria-labelledby={titleId} className="grid gap-2">
      <h2 id={titleId} className="text-base font-semibold">
        Recent reviews
      </h2>
      {error !== null ? (
        <StatusMessage kind="error">Recent reviews could not be loaded. {error}</StatusMessage>
      ) : analyses === undefined ? (
        <StatusMessage kind="loading">Loading recent reviews…</StatusMessage>
      ) : analyses.length === 0 ? (
        <StatusMessage kind="empty">
          No reviews yet. Paste a PR URL above to start. Analyses you start will be listed here.
        </StatusMessage>
      ) : (
        <ul className="grid divide-y divide-border rounded-lg border border-border bg-surface">
          {analyses.map((analysis) => {
            const href = hrefOf(analysis);
            return (
              <li key={analysis.id} className="grid min-w-0 gap-0.5 px-3.5 py-2.5">
                <a
                  href={href}
                  onClick={navigateOnClick(href, onNavigate)}
                  className="min-w-0 break-words font-medium"
                >
                  <span className="whitespace-nowrap">
                    {analysis.pr.repo.owner}/{analysis.pr.repo.name} #{analysis.pr.number}
                  </span>{" "}
                  <span className="text-muted" aria-hidden="true">
                    —
                  </span>{" "}
                  {analysis.title}
                </a>
                <p className="text-[13px] text-muted">
                  {STATUS_TEXT[analysis.status]} · requested{" "}
                  <time dateTime={analysis.created_at} title={dateTimeText(analysis.created_at)}>
                    {agoText(analysis.created_at, now)}
                  </time>
                </p>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
