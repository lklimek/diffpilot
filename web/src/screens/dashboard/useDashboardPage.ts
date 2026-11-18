// Everything the dashboard screen arranges: data, view state, filter and actions (screens only arrange).
import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ApiError } from "../../api/client";
import { useMe } from "../../api/queries";
import type { RefreshStatus } from "../../api/generated/types";
import { useNow } from "../../app/now";
import {
  SUMMARY_KEY,
  useCancelRefresh,
  useDashboardItems,
  useDashboardSummary,
  useRankAgain,
  useRefreshEvents,
  useSaveBrowserTimeZone,
  useSnapshotRefresh,
  useStartRefresh,
} from "./queries";
import { dashboardView } from "./state";

/** localStorage key of the list's kind filter. */
export const FILTER_KEY = "sr.dashboard.kind";
/** How long the "Ranking finished" notice stays. */
export const NOTICE_MS = 6000;

/** The list's kind switch: pull requests, issues or both (TODOs are listed with each). */
export type KindFilter = "pr" | "issue" | "both";

const FILTERS: readonly KindFilter[] = ["pr", "issue", "both"];

function storedFilter(): KindFilter {
  try {
    const value = localStorage.getItem(FILTER_KEY);
    return FILTERS.find((f) => f === value) ?? "pr";
  } catch {
    return "pr";
  }
}

/** The kind filter, remembered across visits; the list opens with pull requests. */
function useKindFilter(): [KindFilter, (filter: KindFilter) => void] {
  const [filter, setFilter] = useState(storedFilter);
  const set = useCallback((next: KindFilter) => {
    setFilter(next);
    try {
      localStorage.setItem(FILTER_KEY, next);
    } catch {
      // Not remembered without storage; the choice still applies.
    }
  }, []);
  return [filter, set];
}

const LIVE_RANKING = ["running", "waiting_for_analysis", "paused"];

/** Calls `report` with the item count when the ranking of refresh `id` goes from live to `finished`. */
function useFinishedRanking(status: RefreshStatus | null | undefined, report: (total: number) => void): void {
  const id = status?.id;
  const state = status?.ranking?.state;
  const total = status?.ranking?.total;
  const seen = useRef<{ id?: string; state?: string }>({});
  useEffect(() => {
    const before = seen.current;
    seen.current = { id, state };
    if (before.id === id && before.state !== undefined && LIVE_RANKING.includes(before.state)) {
      if (state === "finished" && total !== undefined) report(total);
    }
  }, [id, state, total, report]);
}

/** "Ranking finished" for a while after a ranking this page watched reached `finished` (plan §B.3 B-S5). */
function useRankingFinished(latest?: RefreshStatus | null, holder?: RefreshStatus | null): string | null {
  const [notice, setNotice] = useState<string | null>(null);
  const report = useCallback(
    (n: number) => setNotice(`Ranking finished: ${n} ${n === 1 ? "item" : "items"} judged.`),
    [],
  );
  useFinishedRanking(latest, report);
  useFinishedRanking(holder, report);
  useEffect(() => {
    if (notice === null) return;
    const timer = setTimeout(() => setNotice(null), NOTICE_MS);
    return () => clearTimeout(timer);
  }, [notice]);
  return notice;
}

/** Plain text of a failed action. */
function actionError(error: unknown, action: "refresh" | "cancel" | "rank"): string | null {
  if (!error) return null;
  if (error instanceof ApiError) {
    if (error.code === "refresh_running") {
      return action === "rank"
        ? "A refresh is running; rank again after it finished."
        : "A refresh is already scanning; its progress is shown.";
    }
    if (error.code === "conflict") {
      return action === "cancel"
        ? "This refresh had already ended."
        : "This ranking can no longer be retried.";
    }
    return error.message;
  }
  return "Something went wrong. Try again.";
}

/** The dashboard page: signed-in user, summary, items, view state, filter and actions. */
export function useDashboardPage() {
  const client = useQueryClient();
  const me = useMe();
  const user = me.data?.user ?? null;
  const summary = useDashboardSummary(user !== null);
  const data = summary.data;
  const snapshotRefresh = useSnapshotRefresh(data);
  const items = useDashboardItems(data?.revision, data?.snapshot != null);
  useRefreshEvents(data?.latest_refresh);
  useRefreshEvents(snapshotRefresh.data);
  useSaveBrowserTimeZone(data);
  const now = useNow();
  const [filter, setFilter] = useKindFilter();
  const start = useStartRefresh();
  const cancel = useCancelRefresh();
  const rank = useRankAgain();
  const notice = useRankingFinished(data?.latest_refresh, snapshotRefresh.data);

  // An action's error belongs to the refresh it was about: a change of the latest refresh clears it.
  const latestKey = `${data?.latest_refresh?.id}:${data?.latest_refresh?.state}`;
  const { reset: resetStart } = start;
  const { reset: resetCancel } = cancel;
  const { reset: resetRank } = rank;
  useEffect(() => {
    resetStart();
    resetCancel();
    resetRank();
  }, [latestKey, resetStart, resetCancel, resetRank]);

  // Items of a newer revision than the summary: the summary is behind, read it again.
  const itemsRevision = items.data?.revision;
  useEffect(() => {
    if (data && itemsRevision !== undefined && itemsRevision !== data.revision && !items.isPlaceholderData) {
      void client.invalidateQueries({ queryKey: SUMMARY_KEY });
    }
  }, [client, data, itemsRevision, items.isPlaceholderData]);

  const view = useMemo(() => data && dashboardView(data, snapshotRefresh.data), [data, snapshotRefresh.data]);
  const nowDate = useMemo(() => new Date(now), [now]);

  return {
    me,
    user,
    summary,
    items,
    view,
    /** All items of the snapshot in rank order. */
    allItems: items.data?.items,
    filter,
    setFilter,
    now: nowDate,
    notice,
    refresh: {
      start: () => start.mutate(),
      starting: start.isPending,
      error: actionError(start.error, "refresh"),
    },
    cancel: {
      run: (id: string) => cancel.mutate(id),
      pending: cancel.isPending,
      error: actionError(cancel.error, "cancel"),
    },
    rankAgain: {
      run: (id: string) => rank.mutate(id),
      pending: rank.isPending,
      error: actionError(rank.error, "rank"),
    },
  };
}
