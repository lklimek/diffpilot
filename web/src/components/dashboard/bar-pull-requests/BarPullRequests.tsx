import { useId } from "react";
import { HideButton } from "../../button/HideButton";
import { GithubMark } from "../../github/GithubMark";
import { Tooltip } from "../../tooltip/Tooltip";

/** One pull request behind a bar of the net chart. */
export interface BarPullRequest {
  /** `owner/name#number`. */
  id: string;
  /** Its title when the snapshot knows it. */
  title: string | null;
  /** GitHub link; null keeps the id as text. */
  url: string | null;
  /** How a closed one ended; absent for opened ones. */
  outcome?: "merged" | "closed_unmerged";
}

interface BarPullRequestsProps {
  /** The bar's day or week (`Tue 17 Nov`, `Week of 16 Nov (so far)`). */
  period: string;
  opened: BarPullRequest[];
  closed: BarPullRequest[];
  onHide: () => void;
  /** The chart, focused again when Hide is used by keyboard. */
  focusAfter?: () => HTMLElement | SVGElement | null | undefined;
}

const OUTCOME = { merged: "merged", closed_unmerged: "closed without merge" } as const;

/** The pull requests opened and closed in one bar of the net chart, linked to GitHub; Hide closes it. */
export function BarPullRequests({ period, opened, closed, onHide, focusAfter }: BarPullRequestsProps) {
  const titleId = useId();
  const name = `Pull requests of ${period}`;
  return (
    <section
      aria-labelledby={titleId}
      className="flex items-start gap-2 rounded-lg border border-border bg-surface-2 p-3 text-xs"
    >
      <div className="flex min-w-0 flex-1 flex-col gap-2.5">
        <h4 id={titleId} className="text-[13px] font-semibold">
          {name}
        </h4>
        <Group label="Opened" tip="Pull requests you opened" items={opened} />
        <Group label="Closed" tip="Your pull requests merged or closed without merge" items={closed} />
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

function Group({ label, tip, items }: { label: string; tip: string; items: BarPullRequest[] }) {
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
            <li key={`${pr.id} ${pr.outcome ?? ""}`} className="flex min-w-0 items-baseline gap-2">
              {pr.url ? (
                <Tooltip tip={`Open ${pr.id} on GitHub (new tab)`}>
                  <a
                    href={pr.url}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="inline-flex min-h-6 shrink-0 items-center font-mono text-accent underline"
                  >
                    {pr.id}
                    <GithubMark />
                  </a>
                </Tooltip>
              ) : (
                <span className="shrink-0 font-mono">{pr.id}</span>
              )}
              {pr.title && <span className="min-w-0 truncate">{pr.title}</span>}
              {pr.outcome && <span className="shrink-0 text-muted">{OUTCOME[pr.outcome]}</span>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
