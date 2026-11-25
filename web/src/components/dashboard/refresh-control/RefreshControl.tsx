import { useRef } from "react";
import type { RefreshStatus, ScheduleStatus, SnapshotMeta } from "../../../api/generated/types";
import { RANKING_DAILY_LIMIT } from "../../../api/provisional/dashboardJudgement";
import type { DashboardLive } from "../../../api/provisional/dashboardLive";
import { BUSY_NOTE } from "../../button/button";
import { clockText, dateTimeText } from "../../format/time";
import { MenuItem } from "../../menu/MenuButton";
import { SplitButton } from "../../menu/SplitButton";
import { Tooltip } from "../../tooltip/Tooltip";
import { elapsedText } from "../common/format";
import { RefreshIcon, Spinner } from "../common/icons";
import { canRetry } from "../refresh-error/RefreshError";

export interface RankingNotice {
  label: string;
  detail: string;
}

interface RefreshControlProps {
  latest: RefreshStatus | null;
  snapshot: SnapshotMeta | null;
  snapshotRefresh?: RefreshStatus;
  schedule: ScheduleStatus;
  live?: DashboardLive;
  outdated?: number;
  onRefresh: () => void;
  onRejudgeOutdated?: () => void;
  onCancel?: () => void;
  starting?: boolean;
  cancelling?: boolean;
  rankingNotices?: RankingNotice[];
  onRetryRanking?: () => void;
  retryingRanking?: boolean;
  now: Date;
}

function scanAnnouncement(latest: RefreshStatus | null, scannedAt: string | null) {
  switch (latest?.state) {
    case "scanning":
      return "Scanning GitHub and MemCan.";
    case "failed":
      return "The refresh failed.";
    case "cancelled":
      return "The refresh was cancelled.";
    default:
      return scannedAt ? `Refreshed at ${scannedAt}.` : "No full scan yet.";
  }
}

function scheduleProblem(schedule: ScheduleStatus, snapshot: SnapshotMeta | null, now: Date) {
  const run = schedule.last_run;
  if (!run || (run.outcome !== "needs_sign_in" && run.outcome !== "failed")) return null;
  if (snapshot && Date.parse(snapshot.scanned_at) > Date.parse(run.at)) return null;
  const text = run.outcome === "needs_sign_in" ? "needs you to sign in again" : "failed";
  return {
    text: `! Nightly scan at ${clockText(run.at, now)} ${text}`,
    tip:
      run.reason ??
      (run.outcome === "failed" ? "The scheduled scan failed." : "Your GitHub sign-in could not be renewed."),
  };
}

export function RefreshControl({
  latest,
  snapshot,
  snapshotRefresh,
  schedule,
  live,
  outdated = 0,
  onRefresh,
  onRejudgeOutdated,
  onCancel,
  starting = false,
  cancelling = false,
  rankingNotices = [],
  onRetryRanking,
  retryingRanking = false,
  now,
}: RefreshControlProps) {
  const scannedAt = snapshot ? clockText(snapshot.scanned_at, now) : null;
  const own = latest?.id === snapshot?.refresh_id ? latest : snapshotRefresh;
  const coverage =
    live &&
    `${live.live_repositories} live ${live.live_repositories === 1 ? "repository" : "repositories"} · ${live.not_live_repositories} not live. ${live.last_event_at ? `Last GitHub event: ${dateTimeText(live.last_event_at)}.` : "No GitHub events recorded."}`;
  const tip = [
    coverage,
    snapshot ? `Last full scan: ${dateTimeText(snapshot.scanned_at)}.` : "No full scan has finished.",
    own && `LLM judged ${own.judged} · reused ${own.reused}.`,
    ...rankingNotices.map(({ detail }) => detail),
  ]
    .filter(Boolean)
    .join(" ");
  const problem = scheduleProblem(schedule, snapshot, now);
  const scanning = latest?.state === "scanning";
  const limited = latest?.state === "failed" && !canRetry(latest, now) ? latest.failure?.retry_at : null;
  const disabledReason = scanning
    ? "A scan is running."
    : limited
      ? `GitHub allows new requests from ${clockText(limited, now)}.`
      : starting
        ? BUSY_NOTE
        : undefined;
  const label = scanning
    ? `Refreshing… ${elapsedText(now.getTime() - Date.parse(latest.started_at))}`
    : limited
      ? `Retry at ${clockText(limited, now)}`
      : "Refresh";
  const description = `Scan GitHub and MemCan. New items are judged by the LLM.${latest?.state === "ranking" ? " Starting a scan stops the current LLM ranking." : ""}`;
  const scanText = scannedAt ? `scan ${scannedAt}` : "no full scan yet";
  const status = !live
    ? scannedAt
      ? `Full ${scanText}`
      : "No full scan yet"
    : live.live_repositories > 0
      ? `● Live · ${scannedAt ? `full ${scanText}` : scanText}`
      : `Not live · ${scanText}`;

  const rankingLabel = rankingNotices[0]?.label;
  const statusRef = useRef<HTMLSpanElement>(null);
  const retryDescription =
    "Rank the current snapshot again. Only items that still need judging count toward the limits.";
  const retryDisabled =
    latest?.state === "scanning" || latest?.state === "ranking"
      ? "Wait for the running refresh to finish."
      : starting || retryingRanking
        ? BUSY_NOTE
        : undefined;

  return (
    <div className="flex min-w-0 items-center gap-2.5">
      {problem && (
        <Tooltip tip={problem.tip}>
          <span tabIndex={0} data-schedule-problem className="cursor-help text-xs font-semibold text-med">
            {problem.text}
          </span>
        </Tooltip>
      )}
      <span role="status" className="sr-only">
        {scanAnnouncement(latest, scannedAt)}
        {rankingLabel && ` ${rankingLabel}.`}
      </span>
      <Tooltip tip={tip} className="min-w-0">
        <span
          ref={statusRef}
          data-scan-info
          tabIndex={0}
          className="block cursor-help truncate text-xs text-muted"
        >
          {rankingLabel ? `${rankingLabel} · ${status}` : status}
        </span>
      </Tooltip>
      {onRetryRanking && (
        <Tooltip tip={retryDisabled ?? retryDescription} className="shrink-0">
          <button
            type="button"
            aria-label="Retry ranking"
            aria-disabled={Boolean(retryDisabled) || undefined}
            className="min-h-6 min-w-6 cursor-pointer px-1 text-xs font-semibold text-accent underline aria-disabled:cursor-not-allowed aria-disabled:text-muted"
            onClick={() => {
              if (retryDisabled) return;
              statusRef.current?.focus();
              onRetryRanking();
            }}
          >
            Retry
          </button>
        </Tooltip>
      )}
      <div className="shrink-0">
        <SplitButton
          label={label}
          icon={scanning ? <Spinner /> : <RefreshIcon />}
          moreLabel="More refresh options"
          description={description}
          disabledReason={disabledReason}
          onAction={onRefresh}
        >
          <MenuItem
            label="Refresh"
            description={description}
            disabledReason={disabledReason}
            onSelect={onRefresh}
          />
          <MenuItem
            label={`Refresh and re-judge outdated (${outdated})`}
            description={`Scans, then re-judges outdated items. Uses up to ${outdated} of today’s ${RANKING_DAILY_LIMIT} judgements. The daily and per-refresh caps may leave some items outdated.`}
            disabledReason={
              disabledReason ??
              (outdated === 0
                ? "No outdated judgements."
                : onRejudgeOutdated
                  ? undefined
                  : "Re-judging outdated items is unavailable.")
            }
            onSelect={onRejudgeOutdated}
          />
          {onRetryRanking && (
            <MenuItem
              label="Retry ranking"
              description={retryDescription}
              disabledReason={retryDisabled}
              onSelect={onRetryRanking}
            />
          )}
          {scanning && onCancel && (
            <MenuItem
              label="Cancel scan"
              onSelect={onCancel}
              disabledReason={cancelling ? BUSY_NOTE : undefined}
            />
          )}
        </SplitButton>
      </div>
    </div>
  );
}
