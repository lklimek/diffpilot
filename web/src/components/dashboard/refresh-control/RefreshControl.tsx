import type { ReactNode } from "react";
import type { RefreshStatus, ScheduleStatus, SnapshotMeta } from "../../../api/generated/types";
import { buttonClass, BUSY_NOTE } from "../../button/button";
import { clockText, dateTimeText } from "../../format/time";
import { Tooltip } from "../../tooltip/Tooltip";
import { agoText, durationText, elapsedText } from "../common/format";
import { RefreshIcon, Spinner } from "../common/icons";
import { canRetry } from "../refresh-error/RefreshError";

interface RefreshControlProps {
  /** The newest refresh; null before the first. */
  latest: RefreshStatus | null;
  /** The snapshot the list shows; null before the first scan. */
  snapshot: SnapshotMeta | null;
  /** The daily automatic refresh. */
  schedule: ScheduleStatus;
  onRefresh: () => void;
  /** The start request runs. */
  starting?: boolean;
  now: Date;
}

interface Info {
  text: ReactNode;
  tip: string;
  error?: boolean;
}

const how = (trigger: RefreshStatus["trigger"]) => (trigger === "manual" ? "manually" : "automatically");

/** Who started a refresh, for the scan info's tip. */
const startedBy: Record<RefreshStatus["trigger"], string> = {
  manual: "You started it.",
  scheduled: "The daily schedule started it.",
  automatic: "It started by itself while My work was open: every 30 minutes, and on opening after 15.",
};

function scanInfo(latest: RefreshStatus | null, snapshot: SnapshotMeta | null, now: Date): Info {
  const at = (iso: string) => clockText(iso, now);
  const showing = snapshot ? ` · showing ${at(snapshot.scanned_at)}` : "";
  if (latest?.state === "scanning") {
    return {
      text: (
        <>
          <Spinner /> Scanning… {elapsedText(now.getTime() - new Date(latest.started_at).getTime())}
          {showing}
        </>
      ),
      tip: `A scan is running (usually 1–2 minutes).${snapshot ? ` The list keeps the ${at(snapshot.scanned_at)} snapshot until it finishes.` : ""}`,
    };
  }
  if (latest?.state === "failed" || latest?.state === "cancelled") {
    const verb = latest.state === "failed" ? "failed" : "cancelled";
    return {
      text: `Refresh of ${at(latest.started_at)} ${verb}${showing}`,
      tip: `${latest.failure?.message ?? `The refresh was ${verb}.`} Nothing was replaced.`,
      error: latest.state === "failed",
    };
  }
  if (!snapshot)
    return { text: "Not refreshed yet", tip: "No scan has finished yet. Refresh reads GitHub and MemCan." };
  const own = latest && latest.id === snapshot.refresh_id ? latest : null;
  const trigger = own ? ` ${how(own.trigger)}` : "";
  const started = own
    ? `${dateTimeText(own.started_at)} – ${at(snapshot.scanned_at)} (${durationText(new Date(snapshot.scanned_at).getTime() - new Date(own.started_at).getTime())})`
    : dateTimeText(snapshot.scanned_at);
  const checked = `${snapshot.checked.pr} PRs, ${snapshot.checked.issue} issues and ${snapshot.checked.todo} TODOs checked`;
  const ranked = snapshot.ranked_at ? `; ranked by the LLM at ${at(snapshot.ranked_at)}` : "";
  if (latest?.state === "ranking" && latest.ranking) {
    const { state, judged, total, resumes_at } = latest.ranking;
    const scanned = `The scan finished at ${at(snapshot.scanned_at)}.`;
    if (state === "waiting_for_analysis") {
      return {
        text: `Refreshed ${at(snapshot.scanned_at)} · LLM ranking waits for a review analysis`,
        tip: `${scanned} The LLM ranking starts when a running review analysis finishes; scores stay provisional until then.`,
      };
    }
    if (state === "paused") {
      const until = resumes_at ? ` until ${at(resumes_at)}` : "";
      return {
        text: `Refreshed ${at(snapshot.scanned_at)} · LLM ranking paused${until}`,
        tip: `${scanned} The LLM ranking paused at the usage limit and continues by itself; ${judged} of ${total} items judged so far.`,
      };
    }
    return {
      text: (
        <>
          <Spinner /> Refreshed {at(snapshot.scanned_at)} · LLM ranking {judged} / {total}
        </>
      ),
      tip: `${scanned} The LLM is judging changed and new items; their scores are provisional until then.`,
    };
  }
  const counts = own ? ` · judged ${own.judged} · reused ${own.reused}` : "";
  const factsAt = snapshot.facts_checked_at;
  const facts = factsAt ? ` · PRs checked ${agoText(factsAt, now)}` : "";
  const factsTip = factsAt
    ? ` CI, reviews and merge state of the listed pull requests were read from GitHub at ${at(factsAt)}; they are checked every 5 minutes while My work is open.`
    : "";
  return {
    text: `Refreshed${trigger} at ${at(snapshot.scanned_at)} · ${agoText(snapshot.scanned_at, now)}${facts}${counts}`,
    tip: `Last scan: ${started}.${own ? ` ${startedBy[own.trigger]}` : ""} ${checked}${ranked}.${own ? ` The LLM judged ${own.judged} items and reused ${own.reused} earlier judgements whose inputs did not change.` : ""}${factsTip}`,
  };
}

/** What the live region says: the refresh's state only, without relative times or counts (WCAG 4.1.3). */
function announcement(latest: RefreshStatus | null, snapshot: SnapshotMeta | null, now: Date): string {
  switch (latest?.state) {
    case "scanning":
      return "Scanning GitHub and MemCan.";
    case "failed":
      return "The refresh failed.";
    case "cancelled":
      return "The refresh was cancelled.";
    case "ranking": {
      const ranking = latest.ranking;
      if (ranking?.state === "waiting_for_analysis")
        return "The scan finished; the LLM ranking waits for a review analysis.";
      if (ranking?.state === "paused")
        return `The scan finished; the LLM ranking is paused${ranking.resumes_at ? ` until ${clockText(ranking.resumes_at, now)}` : ""}.`;
      return "The scan finished; the LLM is ranking.";
    }
    default:
      return snapshot ? `Refreshed at ${clockText(snapshot.scanned_at, now)}.` : "Not refreshed yet.";
  }
}

/** The scheduled run's failure, shown until a later scan succeeds; null otherwise. */
function scheduleProblem(schedule: ScheduleStatus, snapshot: SnapshotMeta | null, now: Date): Info | null {
  const run = schedule.last_run;
  if (!run || (run.outcome !== "needs_sign_in" && run.outcome !== "failed")) return null;
  if (snapshot && new Date(snapshot.scanned_at) > new Date(run.at)) return null;
  const at = clockText(run.at, now);
  return run.outcome === "needs_sign_in"
    ? {
        text: `Automatic refresh at ${at} needs you to sign in again`,
        tip:
          run.reason ??
          "Your GitHub sign-in could not be renewed. Sign in again; Refresh works once you have.",
      }
    : { text: `Automatic refresh at ${at} failed`, tip: run.reason ?? "The scheduled refresh failed." };
}

/** Scan info and the Refresh button of the top bar (mockup `#scaninfo`, `#refresh`; plan §A.4, §A.5, §B.4). */
export function RefreshControl({
  latest,
  snapshot,
  schedule,
  onRefresh,
  starting = false,
  now,
}: RefreshControlProps) {
  const info = scanInfo(latest, snapshot, now);
  const problem = scheduleProblem(schedule, snapshot, now);
  const scanning = latest?.state === "scanning";
  const limited = latest?.state === "failed" && !canRetry(latest, now) ? latest.failure!.retry_at! : null;
  const disabled = scanning || limited !== null || starting;
  const label = scanning ? "Refreshing…" : limited ? `Retry at ${clockText(limited, now)}` : "Refresh";
  const tip = scanning
    ? "A scan is running; wait for it to finish or cancel it."
    : limited
      ? `GitHub allows new requests from ${clockText(limited, now)}. Retrying earlier would fail again.`
      : `Scan GitHub and MemCan again (about 1–2 minutes), then let the LLM rank changed items. Read-only: nothing is written to GitHub.${
          latest?.state === "ranking" ? " The ranking running now stops; the new scan ranks again." : ""
        }${starting ? ` ${BUSY_NOTE}` : ""}`;
  return (
    <div className="flex min-w-0 items-center gap-3">
      {problem && (
        <Tooltip tip={problem.tip}>
          <span
            tabIndex={0}
            data-schedule-problem
            className="cursor-help truncate text-[12.5px] font-semibold text-med"
          >
            {problem.text}
          </span>
        </Tooltip>
      )}
      <span role="status" className="sr-only">
        {announcement(latest, snapshot, now)}
      </span>
      <Tooltip tip={info.tip}>
        <span
          data-scan-info
          tabIndex={0}
          className={`flex cursor-help items-center gap-1.5 truncate text-[12.5px] ${info.error ? "text-high" : "text-muted"}`}
        >
          {info.text}
        </span>
      </Tooltip>
      <Tooltip tip={tip}>
        <button
          type="button"
          aria-disabled={disabled || undefined}
          onClick={() => !disabled && onRefresh()}
          className={buttonClass("plain", true)}
        >
          <RefreshIcon />
          {label}
        </button>
      </Tooltip>
    </div>
  );
}
