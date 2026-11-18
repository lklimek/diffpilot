// What the dashboard shows for a summary (docs/contracts/http-api.md "Refresh states", plan §A.5, §B.3, §B.4):
// pure functions of the screen, tested without rendering.
import type {
  DashboardSummary,
  RefreshId,
  RefreshStatus,
  RefreshTrigger,
  SnapshotMeta,
  WorkItem,
} from "../../api/generated/types";
import { reviewLabel } from "../home/outcome";

/** State of the Refresh button. */
export type RefreshButtonState =
  | { state: "ready" }
  /** A scan runs: a second start would be refused (409 `refresh_running`). */
  | { state: "scanning" }
  /** GitHub's rate limit: a new refresh fails again before `retryAt`. */
  | { state: "blocked"; retryAt: string };

/** The ranking banner of the current snapshot (plan §B.3); null when ranked or nothing to say. */
export type RankingNotice =
  | { kind: "waiting_for_analysis"; judged: number; total: number }
  | { kind: "running"; judged: number; total: number }
  | { kind: "paused"; judged: number; total: number; resumesAt: string | null }
  | {
      kind: "stopped";
      state: "failed" | "interrupted" | "cancelled";
      message: string | null;
      /** The refresh whose ranking "Rank again" (R4) restarts. */
      refreshId: RefreshId;
      /** False while another refresh of the user is live (R4 would answer 409 `refresh_running`). */
      canRankAgain: boolean;
    };

/** Scan info of the shown snapshot; fields of its refresh are null when that refresh's status is unknown. */
export interface ScanInfo {
  trigger: RefreshTrigger | null;
  scannedAt: string;
  rankedAt: string | null;
  judged: number | null;
  reused: number | null;
}

/** A schedule or limit note, shown as a banner. */
export interface DashboardNote {
  id: "time_zone" | "schedule" | "left_provisional" | "older_prompt";
  tone: "info" | "warn";
  title: string;
  text: string;
}

/** Everything the dashboard shows about its refreshes. */
export interface DashboardView {
  /** The shown snapshot; null before the first successful scan. */
  snapshot: SnapshotMeta | null;
  /** The refresh that is scanning, for the progress card. */
  scanning: RefreshStatus | null;
  /** The latest refresh when its scan failed (failure, steps and log). */
  failure: RefreshStatus | null;
  ranking: RankingNotice | null;
  refreshButton: RefreshButtonState;
  scanInfo: ScanInfo | null;
  /** The list is a previous snapshot while a scan runs or after it failed. */
  stale: boolean;
  /** A snapshot in which nothing needs the user. */
  empty: boolean;
  notes: DashboardNote[];
}

const isLive = (r: RefreshStatus) => r.state === "scanning" || r.state === "ranking";
const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

function rankingNotice(refresh: RefreshStatus, otherLive: boolean): RankingNotice | null {
  const r = refresh.ranking;
  if (!r) return null;
  switch (r.state) {
    case "waiting_for_analysis":
    case "running":
      return { kind: r.state, judged: r.judged, total: r.total };
    case "paused":
      return { kind: "paused", judged: r.judged, total: r.total, resumesAt: r.resumes_at };
    case "failed":
    case "interrupted":
    case "cancelled":
      return {
        kind: "stopped",
        state: r.state,
        message: r.message,
        refreshId: refresh.id,
        canRankAgain: refresh.state === "finished" && !otherLive,
      };
    default:
      return null;
  }
}

function scheduleNotes(summary: DashboardSummary): DashboardNote[] {
  const { schedule, latest_refresh: latest } = summary;
  if (!schedule.enabled) return [];
  if (schedule.time_zone === null) {
    return [
      {
        id: "time_zone",
        tone: "info",
        title: `The daily refresh at ${schedule.time} starts once your time zone is known.`,
        text: "This page saves your browser's time zone; you can change it in Settings.",
      },
    ];
  }
  const run = schedule.last_run;
  if (!run || (run.outcome !== "needs_sign_in" && run.outcome !== "failed")) return [];
  // Shown until a later refresh succeeds (a scan that stored its snapshot).
  const succeededLater =
    latest !== null &&
    (latest.state === "ranking" || latest.state === "finished") &&
    Date.parse(latest.started_at) > Date.parse(run.at);
  if (succeededLater) return [];
  return [
    {
      id: "schedule",
      tone: "warn",
      title:
        run.outcome === "needs_sign_in"
          ? `Automatic refresh at ${schedule.time} needs you to sign in again.`
          : `Automatic refresh at ${schedule.time} failed.`,
      text: run.reason ?? "Refresh by hand meanwhile; the list keeps the last snapshot.",
    },
  ];
}

function limitNotes(summary: DashboardSummary, holder: RefreshStatus | undefined): DashboardNote[] {
  const notes: DashboardNote[] = [];
  const left = holder?.left_provisional ?? 0;
  if (left > 0) {
    notes.push({
      id: "left_provisional",
      tone: "info",
      title: `${plural(left, "item", "items")} left provisional: daily ranking limit.`,
      text: "They keep approximate scores until a later refresh ranks them.",
    });
  }
  if (summary.older_prompt_judgements > 0) {
    notes.push({
      id: "older_prompt",
      tone: "info",
      title: `${plural(summary.older_prompt_judgements, "item", "items")} judged with an older ranking prompt.`,
      text: "They are judged again within the ranking limits.",
    });
  }
  return notes;
}

/**
 * The dashboard's view of `summary` at `now` (ms). `snapshotRefresh` is the status of the refresh holding the
 * snapshot when that is not the latest refresh (R2b); without it, its ranking and counts are unknown.
 */
export function dashboardView(
  summary: DashboardSummary,
  snapshotRefresh: RefreshStatus | undefined,
  now: number,
): DashboardView {
  const latest = summary.latest_refresh;
  const snapshot = summary.snapshot;
  const holder =
    snapshot === null
      ? undefined
      : latest?.id === snapshot.refresh_id
        ? latest
        : snapshotRefresh?.id === snapshot.refresh_id
          ? snapshotRefresh
          : undefined;
  const scanning = latest?.state === "scanning" ? latest : null;
  const failure = latest?.state === "failed" ? latest : null;
  const otherLive = latest !== null && latest.id !== holder?.id && isLive(latest);
  const retryAt = failure?.failure?.retry_at ?? null;
  return {
    snapshot,
    scanning,
    failure,
    // While a new refresh scans, it supersedes the snapshot's ranking: only the scan is shown.
    ranking: holder && !scanning ? rankingNotice(holder, otherLive) : null,
    refreshButton: scanning
      ? { state: "scanning" }
      : retryAt !== null && Date.parse(retryAt) > now
        ? { state: "blocked", retryAt }
        : { state: "ready" },
    scanInfo: snapshot && {
      trigger: holder?.trigger ?? null,
      scannedAt: snapshot.scanned_at,
      rankedAt: snapshot.ranked_at,
      judged: holder?.judged ?? null,
      reused: holder?.reused ?? null,
    },
    stale: snapshot !== null && (scanning !== null || failure !== null),
    empty: snapshot !== null && snapshot.item_counts.total === 0,
    notes: [...scheduleNotes(summary), ...limitNotes(summary, holder)],
  };
}

/** The list's kind filter: the UI opens with pull requests (http-api.md "Items"). */
export type KindFilter = "pr" | "issue" | "both";

/** Items of `filter`; TODOs are listed whatever the kind (as R1b's `kind` filter). */
export const filterItems = (items: readonly WorkItem[], filter: KindFilter): WorkItem[] =>
  items.filter((i) => filter === "both" || i.kind === filter || i.kind === "todo");

/** Where a PR row's review action goes (plan §F.1); null for issues and TODOs. */
export interface ReviewAction {
  /** `new`: no analysis; `found`: one of the current head; `moved`: the PR moved since. */
  kind: "new" | "found" | "moved";
  href: string;
  label: string;
}

/** The review action of a pull request item. */
export function reviewAction(item: WorkItem): ReviewAction | null {
  if (item.kind !== "pr" || item.url === null) return null;
  const review = item.review;
  if (review && !review.head_moved) {
    return { kind: "found", href: `/analyses/${review.analysis_id}`, label: reviewLabel(review) };
  }
  return {
    kind: review ? "moved" : "new",
    href: `/review?pr=${encodeURIComponent(item.url)}`,
    label: "Review",
  };
}
