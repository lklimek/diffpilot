import { Tooltip } from "../tooltip/Tooltip";
import type { WatchedPr } from "../../api/generated/types";
import type { ReviewCommentsSummary } from "../../api/provisional/comments";
import { WatchControl } from "../watch/WatchControl";
import { assessmentText } from "./AssessmentBanner";
import { buttonClass } from "../button/button";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";
import { clockText } from "../format/time";
import { CollapsibleSection } from "../collapsible/CollapsibleSection";
import { FactChip, READ_FAILURE } from "./parts";

interface ProcessCommentsCardProps {
  summary: ReviewCommentsSummary;
  watch?: WatchedPr | null;
  watchBusy?: boolean;
  onWatchChange?: (watching: boolean) => void;
  now: Date;
  questionsHref: string;
  tableHref: string;
  onNavigate: Navigate;
  /** Where the section's open state is remembered. */
  storageKey: string;
}

/** V0 "Process PR comments": GitHub facts, questions, agents, assessment, round, and the way in (US1). */
export function ProcessCommentsCard({
  summary,
  now,
  watch = null,
  watchBusy,
  onWatchChange,
  questionsHref,
  tableHref,
  onNavigate,
  storageKey,
}: ProcessCommentsCardProps) {
  const { counts, read } = summary;
  return (
    <div className="rounded-lg border border-border bg-surface px-4 pb-3.5">
      <CollapsibleSection title="Process PR comments" level={2} storageKey={storageKey}>
        <dl className="m-0 grid grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)] sm:grid-cols-[max-content_minmax(0,1fr)] gap-x-3 gap-y-1 text-[13px]">
          <dt className="text-muted">Unresolved threads on GitHub</dt>
          <dd className="m-0">
            {counts.unresolved_threads}{" "}
            {read.read_at && (
              <FactChip
                text={`read ${clockText(read.read_at, now)}`}
                tip={`Read from GitHub at ${clockText(read.read_at, now)}.`}
              />
            )}
            {read.failure && (
              <p role="status" className="m-0 text-xs text-high">
                GitHub could not be read ({READ_FAILURE[read.failure] ?? read.failure}).{" "}
                {read.read_at ? "The last read stays shown." : "Nothing was read from GitHub yet."}
              </p>
            )}
          </dd>
          <dt className="text-muted">Needs you</dt>
          <dd className="m-0">
            <b>{counts.undecided}</b>
            {counts.asked_again > 0 && ` (${counts.asked_again} asked again)`}
          </dd>
          <dt className="text-muted">Waiting for agents</dt>
          <dd className="m-0">
            {counts.waiting_for_agents} {counts.waiting_for_agents === 1 ? "decision" : "decisions"}
            {counts.failed_effects > 0 && ` · ${counts.failed_effects} failed`}
          </dd>
          <dt className="text-muted">LLM assessment</dt>
          <dd className="m-0">{assessmentText(summary.assessment, now)}</dd>
          <dt className="text-muted">Round</dt>
          <dd className="m-0">
            {summary.rounds === 0 ? "no bot round yet" : `${summary.rounds} (thepastaclaw)`}
          </dd>
        </dl>
        <div className="mt-3 flex flex-wrap items-end gap-2">
          <Tooltip tip="Opens the questions on this pull request's review comments, one at a time.">
            <a
              href={questionsHref}
              onClick={navigateOnClick(questionsHref, onNavigate)}
              className={buttonClass("primary")}
            >
              Open Comments →
            </a>
          </Tooltip>
          <Tooltip tip="Opens every comment in one table with a decision per row.">
            <a
              href={tableHref}
              onClick={navigateOnClick(tableHref, onNavigate)}
              className={buttonClass("plain")}
            >
              Table
            </a>
          </Tooltip>
          {onWatchChange && (
            <div className="ml-auto">
              <WatchControl
                variant="button"
                watch={watch}
                busy={watchBusy}
                onChange={onWatchChange}
                now={now}
              />
            </div>
          )}
        </div>
      </CollapsibleSection>
    </div>
  );
}
