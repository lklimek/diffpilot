import { useId } from "react";
import { HideButton } from "../../button/HideButton";
import { Tooltip } from "../../tooltip/Tooltip";
import { OpenExternal } from "../common/OpenExternal";
import { WorkTitleLink } from "../common/WorkTitleLink";
import type { Navigate } from "../common/navigate";

/** One pull request behind a bar of the net chart. */
export interface BarPullRequest {
  /** `owner/name#number`. */
  id: string;
  /** Its title when the snapshot knows it. */
  title: string | null;
  /** DiffPilot PR page, when the PR identity is valid. */
  href?: string;
  /** GitHub link; null keeps the id as text. */
  url: string | null;
  /** How a closed one ended; absent for opened ones. */
  outcome?: "merged" | "closed_unmerged";
}

interface BarPullRequestsProps {
  /** The bar's day or week (`Tue 17 Nov`, `Week of 16 Nov (so far)`). */
  period: string;
  unit?: "day" | "week";
  opened: BarPullRequest[];
  closed: BarPullRequest[];
  onNavigate?: Navigate;
  onHide: () => void;
  /** The chart, focused again when Hide is used by keyboard. */
  focusAfter?: () => HTMLElement | SVGElement | null | undefined;
}

const OUTCOME = { merged: "merged", closed_unmerged: "closed without merge" } as const;

/** The pull requests opened and closed in one bar of the net chart, linked to GitHub; Hide closes it. */
export function BarPullRequests({
  period,
  unit = "week",
  opened,
  closed,
  onNavigate,
  onHide,
  focusAfter,
}: BarPullRequestsProps) {
  const titleId = useId();
  const name = `Pull requests of ${period}`;
  const openedIds = new Set(opened.map((pr) => pr.id.toLowerCase()));
  const closedIds = new Set(closed.map((pr) => pr.id.toLowerCase()));
  const groups =
    unit === "day"
      ? [
          {
            label: "Opened and closed",
            tip: "Pull requests you opened and closed on this day",
            items: closed.filter((pr) => openedIds.has(pr.id.toLowerCase())),
          },
          {
            label: "Opened only",
            tip: "Pull requests you opened on this day that did not close that day",
            items: opened.filter((pr) => !closedIds.has(pr.id.toLowerCase())),
          },
          {
            label: "Closed from earlier days",
            tip: "Your pull requests opened earlier and merged or closed on this day",
            items: closed.filter((pr) => !openedIds.has(pr.id.toLowerCase())),
          },
        ]
      : [
          { label: "Opened", tip: "Pull requests you opened", items: opened },
          { label: "Closed", tip: "Your pull requests merged or closed without merge", items: closed },
        ];
  return (
    <section
      aria-labelledby={titleId}
      className="flex items-start gap-2 rounded-lg border border-border bg-surface-2 p-3 text-xs"
    >
      <div className="flex min-w-0 flex-1 flex-col gap-2.5">
        <h4 id={titleId} className="text-[13px] font-semibold">
          {name}
        </h4>
        {groups.map((group) => (
          <Group key={group.label} {...group} onNavigate={onNavigate} />
        ))}
      </div>
      <HideButton
        name={`Hide pull requests of ${period}`}
        tip="Hides this list; choose its bar in the chart to show it again."
        onHide={onHide}
        focusAfter={focusAfter}
      />
    </section>
  );
}

function Group({
  label,
  tip,
  items,
  onNavigate,
}: {
  label: string;
  tip: string;
  items: BarPullRequest[];
  onNavigate?: Navigate;
}) {
  const heading = `${label} (${items.length})`;
  return (
    <div role="group" aria-label={heading}>
      <Tooltip tip={tip}>
        <p tabIndex={0} className="inline cursor-help font-semibold text-text">
          {heading}
        </p>
      </Tooltip>
      {items.length === 0 ? (
        <p className="text-muted">None.</p>
      ) : (
        <ul className="m-0 mt-1 flex list-none flex-col gap-1 p-0">
          {items.map((pr) => (
            <li key={`${pr.id} ${pr.outcome ?? ""}`} className="flex min-h-6 min-w-0 items-baseline gap-2">
              <WorkTitleLink
                href={pr.title ? undefined : pr.href}
                onNavigate={onNavigate}
                className="max-w-[45%] shrink-0 font-mono break-all"
              >
                {pr.id}
              </WorkTitleLink>
              {(pr.title || pr.url) && (
                <span className="inline-flex min-w-0 items-baseline gap-1.5">
                  {pr.title && (
                    <WorkTitleLink
                      href={pr.href}
                      onNavigate={onNavigate}
                      className="min-w-0 truncate leading-6"
                    >
                      {pr.title}
                    </WorkTitleLink>
                  )}
                  <OpenExternal href={pr.url} label={`Open ${pr.id} on GitHub`} />
                </span>
              )}
              {pr.outcome && <span className="min-w-0 text-muted">{OUTCOME[pr.outcome]}</span>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
