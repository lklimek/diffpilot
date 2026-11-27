// Everything the dashboard screen arranges: data, view state, filter and actions (screens only arrange).
import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ApiError } from "../../api/client";
import { useMe } from "../../api/queries";
import type { RefreshStatus } from "../../api/generated/types";
import { useNow } from "../../app/now";
import { ALL_KINDS, type ItemKind } from "../../components/dashboard/work-list/WorkList";
import {
  SUMMARY_KEY,
  useCancelRefresh,
  useHideMention,
  useSnoozeItem,
  useWakeItem,
  useWatchItem,
  useDashboardItems,
  useDashboardSummary,
  useRejudgeItem,
  useRetryRanking,
  useRefreshEvents,
  useSaveBrowserTimeZone,
  useSlackNames,
  useSnapshotRefresh,
  useStartRefresh,
} from "./queries";
import { useMentionBots, useMentionsExpanded } from "./mentions";
import { dashboardView } from "./state";

/** localStorage key of the kinds shown (a JSON array such as `["pr","todo"]`). */
export const FILTER_KEY = "sr.dashboard.kinds";
/** localStorage key of the earlier single-choice switch, read once when `FILTER_KEY` is absent. */
export const LEGACY_FILTER_KEY = "sr.dashboard.kind";
/** How long the "Ranking finished" notice stays. */
export const NOTICE_MS = 6000;

/** Values of the earlier switch and what they showed (TODOs were listed with each). */
const LEGACY = new Map<string, ItemKind[]>([
  ["pr", ["pr", "todo"]],
  ["issue", ["issue", "todo"]],
  ["both", ALL_KINDS],
]);

/** The known values of a stored JSON array, or null when it is not one or names none. */
function parseChoice<T extends string>(value: string, all: T[]): T[] | null {
  try {
    const parsed: unknown = JSON.parse(value);
    if (!Array.isArray(parsed)) return null;
    const chosen = all.filter((one) => parsed.includes(one));
    return chosen.length > 0 ? chosen : null;
  } catch {
    return null;
  }
}

/** The stored kinds, else the earlier switch's value, else every kind. */
function storedFilter(): ItemKind[] {
  try {
    const value = localStorage.getItem(FILTER_KEY);
    if (value !== null) return parseChoice(value, ALL_KINDS) ?? ALL_KINDS;
    return LEGACY.get(localStorage.getItem(LEGACY_FILTER_KEY) ?? "") ?? ALL_KINDS;
  } catch {
    return ALL_KINDS;
  }
}

/** The kinds shown, remembered across visits; the list opens with every kind. */
function useKindFilter(): [ItemKind[], (filter: ItemKind[]) => void] {
  const [filter, setFilter] = useState(storedFilter);
  const set = useCallback((next: ItemKind[]) => {
    setFilter(next);
    try {
      localStorage.setItem(FILTER_KEY, JSON.stringify(next));
      localStorage.removeItem(LEGACY_FILTER_KEY);
    } catch {
      // Not remembered without storage; the choice still applies.
    }
  }, []);
  return [filter, set];
}

const LIVE_RANKING = ["running", "waiting_for_analysis", "paused"];

/** Calls `report` with the item count when the ranking of refresh `id` goes from live to `finished`. */
function useFinishedRanking(
  status: RefreshStatus | null | undefined,
  report: (judged: number) => void,
): void {
  const id = status?.id;
  const state = status?.ranking?.state;
  const judged = status?.ranking?.judged;
  const seen = useRef<{ id?: string; state?: string }>({});
  useEffect(() => {
    const before = seen.current;
    seen.current = { id, state };
    if (before.id === id && before.state !== undefined && LIVE_RANKING.includes(before.state)) {
      if (state === "finished" && judged !== undefined) report(judged);
    }
  }, [id, state, judged, report]);
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
        ? "Wait for the running refresh to finish before retrying ranking."
        : "A refresh is already scanning; its progress is shown.";
    }
    if (error.code === "conflict") {
      if (action === "cancel") return "This refresh had already ended.";
      if (action === "rank") return "This ranking can no longer be retried.";
    }
    return error.message;
  }
  return "Something went wrong. Try again.";
}

/** The dashboard page: signed-in user, summary, items, view state, filter and actions. */
export function useDashboardPage() {
  const hideMention = useHideMention();
  const snooze = useSnoozeItem();
  const wake = useWakeItem();
  const watch = useWatchItem();
  const [withBots, setWithBots] = useMentionBots();
  const [mentionsExpanded, setMentionsExpanded] = useMentionsExpanded();
  const client = useQueryClient();
  const me = useMe();
  const user = me.data?.user ?? null;
  const summary = useDashboardSummary(user !== null);
  const slackNames = useSlackNames(user !== null);
  const data = summary.data;
  const snapshotRefresh = useSnapshotRefresh(data);
  const items = useDashboardItems(data?.revision, data?.snapshot != null);
  useRefreshEvents(data?.latest_refresh);
  useRefreshEvents(snapshotRefresh.data);
  const timeZoneSave = useSaveBrowserTimeZone(data);
  const now = useNow();
  const [filter, setFilter] = useKindFilter();
  const start = useStartRefresh();
  const cancel = useCancelRefresh();
  const rejudge = useRejudgeItem();
  const rank = useRetryRanking();
  const notice = useRankingFinished(data?.latest_refresh, snapshotRefresh.data);

  // An action's error belongs to the refresh it was about: a change of the latest refresh clears it.
  const latestKey = `${data?.latest_refresh?.id}:${data?.latest_refresh?.state}`;
  const { reset: resetStart } = start;
  const { reset: resetCancel } = cancel;
  const { reset: resetRank } = rank;
  useEffect(() => {
    resetStart();
    resetCancel();
  }, [latestKey, resetStart, resetCancel]);
  useEffect(() => resetRank(), [data?.snapshot?.refresh_id, resetRank]);

  // Items of a newer revision than the summary: the summary is behind, read it again.
  const itemsRevision = items.data?.revision;
  useEffect(() => {
    if (data && itemsRevision !== undefined && itemsRevision !== data.revision && !items.isPlaceholderData) {
      void client.invalidateQueries({ queryKey: SUMMARY_KEY });
    }
  }, [client, data, itemsRevision, items.isPlaceholderData]);

  const view = useMemo(() => data && dashboardView(data, timeZoneSave.failed), [data, timeZoneSave.failed]);
  const nowDate = useMemo(() => new Date(now), [now]);

  return {
    hideMention,
    snooze,
    wake,
    watch,
    rejudge,
    snapshotRefresh: snapshotRefresh.data,
    mentionsExpanded,
    setMentionsExpanded,
    withBots,
    setWithBots,
    me,
    user,
    summary,
    /** The Slack names of Settings; undefined until read (no Slack copy then). */
    slackNames: slackNames.data,
    items,
    view,
    /** All items of the snapshot in rank order. */
    allItems: items.data?.items,
    filter,
    setFilter,
    now: nowDate,
    notice,
    refresh: {
      start: () => start.mutate(false),
      rejudgeOutdated: () => start.mutate(true),
      starting: start.isPending,
      error: actionError(start.error, "refresh"),
    },
    retryRanking: {
      run: (id: string) => rank.mutate(id),
      pending: rank.isPending,
      error: actionError(rank.error, "rank"),
    },
    cancel: {
      run: (id: string) => cancel.mutate(id),
      pending: cancel.isPending,
      error: actionError(cancel.error, "cancel"),
    },
  };
}
