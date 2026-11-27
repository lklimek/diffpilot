import type { AnalysisStatus, PrRef } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";
import { clockText, dateTimeText } from "../format/time";
import { GithubIcon } from "../github/GithubMark";
import { prGithubUrl } from "../github/links";
import { MenuButton, MenuItem } from "../menu/MenuButton";
import { Tooltip } from "../tooltip/Tooltip";
import { WATCH_OFFER } from "../watch/WatchControl";

interface Entry {
  id: string;
  pr: PrRef;
  title: string;
  prHref: string;
  reviewHref?: string;
  lastActivityAt: string;
  openedAt?: string;
  archived?: "merged" | "closed";
  requestedAt?: string;
  analysisStatus?: AnalysisStatus;
  headMoved?: boolean;
  watch?: { state: string; since: string; ended_at: string | null };
}

const STATUS: Record<AnalysisStatus, string> = {
  finished: "finished",
  failed: "⚠ failed",
  running: "◔ running",
  planning: "◔ planning",
  paused: "paused",
  cancelled: "cancelled",
};
const BADGE = "rounded border border-border px-1.5 text-xs";
const WATCH_LABEL: Record<string, string> = {
  waiting_for_bot_round: "waiting for bot round",
  job_running: "assessment running",
  limit_reached: "limit reached",
  paused: "watch paused",
  ended: "ended: merged or closed",
};

export function PrHistory({
  rows,
  now,
  busy,
  canWatch,
  onWatch,
  onNavigate,
}: {
  rows: readonly Entry[];
  now: Date;
  busy: boolean;
  canWatch: boolean;
  onWatch: (pr: PrRef, watching: boolean) => void;
  onNavigate: Navigate;
}) {
  return (
    <ul
      aria-label="Your pull requests"
      className="divide-y divide-border rounded-lg border border-border bg-surface"
    >
      {rows.map((row) => {
        const { pr, watch, analysisStatus } = row;
        const name = `${pr.repo.owner}/${pr.repo.name} #${pr.number}`;
        const githubHref = prGithubUrl(pr);
        const watching = Boolean(watch && watch.state !== "ended");
        const failed = analysisStatus === "failed";
        const running = analysisStatus === "running" || analysisStatus === "planning";
        return (
          <li
            key={row.id}
            className="grid gap-1 px-3 py-2 hover:bg-surface-2 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center sm:gap-3"
          >
            <a
              href={row.prHref}
              onClick={navigateOnClick(row.prHref, onNavigate)}
              className="block min-w-0 rounded text-text no-underline [overflow-wrap:anywhere] group"
            >
              <span className="font-mono text-[13px] font-semibold">{name}</span>
              {row.title && <span className="font-semibold group-hover:underline"> · {row.title}</span>}
              <span className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted">
                {!row.openedAt && !row.requestedAt && !watch && (
                  <span>last activity {clockText(row.lastActivityAt, now)}</span>
                )}
                {row.openedAt && (
                  <span>
                    opened <time dateTime={row.openedAt}>{clockText(row.openedAt, now)}</time>
                  </span>
                )}
                {row.requestedAt && (
                  <span>
                    analysis requested{" "}
                    <time dateTime={row.requestedAt}>{clockText(row.requestedAt, now)}</time>
                  </span>
                )}
                {watch && (
                  <span>
                    {watch.ended_at
                      ? `watch ended ${clockText(watch.ended_at, now)}`
                      : `watching since ${clockText(watch.since, now)}`}
                  </span>
                )}
                {analysisStatus && (
                  <span
                    title={row.requestedAt && dateTimeText(row.requestedAt)}
                    className={`${BADGE} ${failed ? "border-high-bd bg-high-bg text-high" : running ? "border-accent bg-accent-bg text-accent" : ""}`}
                  >
                    analysis {STATUS[analysisStatus]}
                  </span>
                )}
                {row.archived && <span className={BADGE}>Archived: {row.archived}</span>}
                {row.headMoved && <span className={BADGE}>head moved</span>}
                {watch && <span className={BADGE}>{WATCH_LABEL[watch.state] ?? "watching"}</span>}
              </span>
            </a>
            <div className="flex flex-wrap items-center gap-1.5">
              <OpenExternal href={githubHref} label={`Open ${row.id} on GitHub, new tab`} />
              {row.reviewHref && (
                <Tooltip tip="Opens the finished code analysis.">
                  <a
                    href={row.reviewHref}
                    aria-label={`Review ${name}`}
                    onClick={navigateOnClick(row.reviewHref, onNavigate)}
                    className={buttonClass("plain", true)}
                  >
                    Review <span aria-hidden="true">→</span>
                  </a>
                </Tooltip>
              )}
              <MenuButton label={`More actions for ${name}`} trigger={<span aria-hidden="true">⋯</span>}>
                <MenuItem
                  label={watching ? "Stop watching" : "Watch"}
                  description={watching ? undefined : WATCH_OFFER}
                  disabledReason={
                    busy ? "Saving a watch change." : canWatch ? undefined : "Watch status is unavailable."
                  }
                  onSelect={() => onWatch(pr, !watching)}
                />
                <MenuItem label="Open on GitHub" href={githubHref} external icon={<GithubIcon />} />
              </MenuButton>
            </div>
          </li>
        );
      })}
    </ul>
  );
}
