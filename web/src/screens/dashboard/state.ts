// What the dashboard shows for a summary (docs/contracts/http-api.md "Refresh states", plan §A.5, §B.3, §B.4):
// pure functions of the screen, tested without rendering.
import type {
  DashboardSummary,
  RankingStatus,
  RefreshId,
  RefreshStatus,
  SnapshotMeta,
  WorkItem,
} from "../../api/generated/types";
import { REVIEW_START_PATH } from "./paths";

/** The ranking banner of the shown snapshot (plan §B.3). */
export interface RankingNotice {
  status: RankingStatus;
  /** The refresh whose ranking "Retry ranking" (R4) restarts. */
  refreshId: RefreshId;
  /** When the snapshot's scan finished. */
  scannedAt: string;
}

/** A schedule or limit note, shown as a banner. */
export interface DashboardNote {
  id: "time_zone" | "left_provisional" | "older_prompt";
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
  /** Null while ranked, while a new scan runs (it supersedes the ranking) or when the ranking is unknown. */
  ranking: RankingNotice | null;
  /** The list is a previous snapshot while a scan runs or after one failed. */
  stale: boolean;
  /** A snapshot in which nothing needs the user. */
  empty: boolean;
  notes: DashboardNote[];
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** The daily refresh cannot run without a time zone (the scheduled run's failure is the refresh control's). */
function timeZoneNotes(summary: DashboardSummary): DashboardNote[] {
  const { schedule } = summary;
  if (!schedule.enabled || schedule.time_zone !== null) return [];
  return [
    {
      id: "time_zone",
      tone: "info",
      title: `The daily refresh at ${schedule.time} starts once your time zone is known.`,
      text: "This page saves your browser's time zone; you can change it in Settings.",
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
 * The dashboard's view of `summary`. `snapshotRefresh` is the status of the refresh holding the snapshot when that
 * is not the latest refresh (R2b); without it, its ranking and counts are unknown.
 */
export function dashboardView(
  summary: DashboardSummary,
  snapshotRefresh: RefreshStatus | undefined,
): DashboardView {
  const latest = summary.latest_refresh;
  const snapshot = summary.snapshot;
  const holder =
    snapshot === null
      ? undefined
      : [latest, snapshotRefresh].find((r): r is RefreshStatus => r?.id === snapshot.refresh_id);
  const scanning = latest?.state === "scanning" ? latest : null;
  const failure = latest?.state === "failed" ? latest : null;
  const ranking = holder?.ranking;
  return {
    snapshot,
    scanning,
    failure,
    ranking:
      snapshot && holder && ranking && ranking.state !== "finished" && !scanning
        ? { status: ranking, refreshId: holder.id, scannedAt: snapshot.scanned_at }
        : null,
    stale: snapshot !== null && (scanning !== null || failure !== null),
    empty: snapshot !== null && snapshot.item_counts.total === 0,
    notes: [...timeZoneNotes(summary), ...limitNotes(summary, holder)],
  };
}

/**
 * Where a pull request row's review action goes (plan §F.1): its review when an analysis of the current head
 * exists, else the review start page with the PR (it says when the PR moved since an analysis).
 */
export function reviewHref(item: WorkItem): string {
  const review = item.review;
  if (review && !review.head_moved) return `/analyses/${review.analysis_id}`;
  return `${REVIEW_START_PATH}?pr=${encodeURIComponent(item.url ?? "")}`;
}
