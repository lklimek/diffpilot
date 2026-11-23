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
import { commentsPath } from "../pr/paths";
import { REVIEW_START_PATH } from "../shell/paths";

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

/**
 * The daily refresh cannot run without a time zone (the scheduled run's failure is the refresh control's);
 * `saveFailed`: this page could not save the browser's zone.
 */
function timeZoneNotes(summary: DashboardSummary, saveFailed: boolean): DashboardNote[] {
  const { schedule } = summary;
  if (!schedule.enabled || schedule.time_zone !== null) return [];
  return [
    {
      id: "time_zone",
      tone: saveFailed ? "warn" : "info",
      title: `The daily refresh at ${schedule.time} starts once your time zone is known.`,
      text: saveFailed
        ? "Your browser's time zone could not be saved; set it in Settings."
        : "This page saves your browser's time zone; you can change it in Settings.",
    },
  ];
}

/**
 * Items left provisional: the server names the ranking limit that applied in the ranking step's detail (this
 * refresh's or today's); a failed ranking's detail is its failure, so no limit is named then.
 */
function limitNotes(summary: DashboardSummary, holder: RefreshStatus | undefined): DashboardNote[] {
  const notes: DashboardNote[] = [];
  const left = holder?.left_provisional ?? 0;
  if (left > 0) {
    const named =
      holder?.ranking?.state === "failed"
        ? null
        : holder?.steps.find((step) => step.key === "ranking")?.detail;
    notes.push({
      id: "left_provisional",
      tone: "info",
      title: named ? `${named}.` : `${plural(left, "item", "items")} left provisional.`,
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
 * is not the latest refresh (R2b); without it, its ranking and counts are unknown. `timeZoneSaveFailed`: R6b
 * refused or failed.
 */
export function dashboardView(
  summary: DashboardSummary,
  snapshotRefresh: RefreshStatus | undefined,
  timeZoneSaveFailed = false,
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
    notes: [...timeZoneNotes(summary, timeZoneSaveFailed), ...limitNotes(summary, holder)],
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

/** In-app href of a pull request row's Comments (owner 2026-11-23). */
export const commentsHref = (item: WorkItem): string =>
  item.repo && item.number !== null ? commentsPath({ repo: item.repo, number: item.number }) : "";

/** Text of the collapsed side panel's rail: `More · waiting 3 · ready 2` (ready left out when not recorded). */
export function railText(snapshot: SnapshotMeta): string {
  const ready = snapshot.merge_state_pending === null ? "" : ` · ready ${snapshot.ready_to_merge.length}`;
  return `More · waiting ${snapshot.awaiting.length}${ready}`;
}
