// What the dashboard shows for a summary (docs/contracts/http-api.md "Refresh states", plan §A.5, §B.3, §B.4):
// pure functions of the screen, tested without rendering.
import { ApiError } from "../../api/client";
import type { DashboardSummary, RefreshStatus, SnapshotMeta, WorkItem } from "../../api/generated/types";
import type { DashboardItem } from "../../api/provisional/dashboard";
import type { RankingNotice } from "../../components/dashboard/refresh-control/RefreshControl";
import { commentsPath } from "../pr/paths";
import { REVIEW_START_PATH } from "../shell/paths";

/** A schedule or limit note, shown as a banner. */
export interface DashboardNote {
  id: "time_zone";
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
  /** The list is a previous snapshot while a scan runs or after one failed. */
  stale: boolean;
  /** A snapshot in which nothing needs the user. */
  empty: boolean;
  notes: DashboardNote[];
}

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
      title: `The nightly scan at ${schedule.time} starts once your time zone is known.`,
      text: saveFailed
        ? "Your browser's time zone could not be saved; set it in GitHub sync."
        : "This page saves your browser's time zone; you can change it in GitHub sync.",
    },
  ];
}

/** Dashboard scan state and the nightly schedule’s time zone notice. */
export function dashboardView(summary: DashboardSummary, timeZoneSaveFailed = false): DashboardView {
  const { latest_refresh: latest, snapshot } = summary;
  const scanning = latest?.state === "scanning" ? latest : null;
  const failure = latest?.state === "failed" ? latest : null;
  return {
    snapshot,
    scanning,
    failure,
    stale: snapshot !== null && (scanning !== null || failure !== null),
    empty: snapshot !== null && snapshot.item_counts.total === 0,
    notes: timeZoneNotes(summary, timeZoneSaveFailed),
  };
}

/** Feedback about ranking the snapshot on screen, including when a later scan failed. */
export function rankingFeedback(summary: DashboardSummary, snapshotRefresh?: RefreshStatus) {
  const holder = [summary.latest_refresh, snapshotRefresh].find(
    (refresh) => refresh != null && refresh.id === summary.snapshot?.refresh_id,
  );
  const ranking = holder?.ranking;
  const notices: RankingNotice[] = [];
  let retryId: string | undefined;
  if (
    holder?.state === "finished" &&
    ranking &&
    ["failed", "interrupted", "cancelled"].includes(ranking.state)
  ) {
    retryId = holder.id;
    const state =
      ranking.state === "cancelled"
        ? "cancelled"
        : ranking.state === "interrupted"
          ? "interrupted"
          : "failed";
    notices.push({
      label: `Ranking ${state}`,
      detail: ranking.message ?? `The LLM ranking was ${state}.`,
    });
  }
  const left = holder?.left_provisional ?? 0;
  // The ranking step names the cap when the legacy counter alone cannot distinguish it.
  const detail = holder?.steps.find(({ key }) => key === "ranking")?.detail ?? "";
  const limit = detail.match(
    /^\d+ items? left provisional: (today's|this refresh's) ranking limit \(\d+\)$/,
  )?.[1];
  const daily = left > 0 && limit === "today's";
  if (left > 0) {
    const estimate = `${left} ${left === 1 ? "item was" : "items were"} left provisional by the last ranking.`;

    if (daily) {
      const reset = `The daily allowance resets at 00:00 (${summary.schedule.time_zone || "UTC"}).`;
      const reached = ranking?.state === "finished" ? "Daily limit reached" : "Daily ranking limit";
      notices.push({ label: `${reached} · resets at 00:00`, detail: `${estimate} ${reset}` });
    } else {
      notices.push({
        label: limit === "this refresh's" ? "Per-refresh ranking limit" : `${left} left provisional`,
        detail: estimate,
      });
    }
  }
  const notJudged = notices.map(({ label, detail }) => `${label}. ${detail}`).join(" ") || undefined;
  const older = summary.older_prompt_judgements;
  const olderPrompt =
    older > 0
      ? `${older} ${older === 1 ? "item was" : "items were"} judged with an older ranking prompt.`
      : undefined;
  if (olderPrompt) notices.push({ label: `${older} judged with older prompt`, detail: olderPrompt });
  return {
    notices,
    retryId,
    legacyDailyCap: daily && ranking?.state === "finished" && ranking.judged === ranking.total,
    statusDescriptions: { not_judged: notJudged },
  };
}

/** A successful daily-limited ranking left unjudged candidates with open parts out of its queue. */
export function withLegacyDailyCap(item: DashboardItem): DashboardItem {
  const asked = item.score.parts.some(
    ({ status, source }) => status === "estimated" || (status === "pending" && source === "llm"),
  );
  return !item.judgement && !item.llm && item.score.state === "unjudged" && asked
    ? { ...item, judgement: { state: "capped", judged_at: null, outdated_because: null } }
    : item;
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

/**
 * Why a Watch from a My work row failed: a private repository (RW1 reads public ones only, so trying again cannot
 * help), the 20-PR limit, or the server's reason with a retry hint.
 */
export function watchErrorText(error: unknown, watching = true): string {
  if (!watching)
    return `Could not stop watching this pull request. Try again. ${error instanceof ApiError ? error.message : ""}`.trim();
  if (error instanceof ApiError && error.code === "private_repository")
    return "Could not watch this pull request: its repository is private, and diffpilot reads public repositories only.";
  if (error instanceof ApiError && error.code === "watch_limit_reached")
    return "Could not watch this pull request: you watch 20 already. Unwatch one in Watched PRs first.";
  const reason = error instanceof ApiError ? error.message : "Something went wrong.";
  return `Could not watch this pull request. Try again. ${reason}`;
}
