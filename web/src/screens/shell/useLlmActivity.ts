import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useSyncExternalStore } from "react";
import { useLocation } from "react-router";
import { api } from "../../api/client";
import type { DashboardSummary, LlmCallOutcome } from "../../api/generated/types";
import { useNow } from "../../app/now";
import { SUMMARY_KEY } from "../dashboard/queries";
import { MY_WORK_PATH } from "./paths";

const POLL_MS = 30_000;

/** Read job activity, with Settings and Codex status on older servers. */
export function useLlmActivity(login: string) {
  const now = new Date(useNow());
  const { pathname } = useLocation();
  const jobs = useQuery({
    queryKey: ["llm-activity", login],
    queryFn: ({ signal }) => api.getLlmActivity(signal),
    retry: false,
    staleTime: (query) => (query.state.data === null ? Infinity : 0),
    refetchInterval: (query) => (query.state.data === null ? false : POLL_MS),
  });
  const activity = jobs.isError ? null : jobs.data;
  const fallback = {
    enabled: activity === null,
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  };
  // A dashboard read wakes the legacy scanner. Reuse the open dashboard's data without reading it here.
  const client = useQueryClient();
  const subscribe = useCallback((notify: () => void) => client.getQueryCache().subscribe(notify), [client]);
  const legacyRanking = useSyncExternalStore(subscribe, () => {
    if (activity !== null || pathname !== MY_WORK_PATH) return null;
    const refresh = client.getQueryData<DashboardSummary>(SUMMARY_KEY)?.latest_refresh;
    return refresh?.state === "ranking" ? refresh.ranking : null;
  });
  const settings = useQuery({
    queryKey: ["settings"],
    queryFn: api.getSettings,
    ...fallback,
  });
  const account = useQuery({
    queryKey: ["codex", "shell", login],
    queryFn: api.getCodex,
    ...fallback,
  });
  // C7 owns both observations when available, including explicit nulls.
  const lastCall = [account.data?.last_call, settings.data?.llm_status].reduce<LlmCallOutcome | null>(
    (latest, call) => (call && (!latest || Date.parse(call.at) > Date.parse(latest.at)) ? call : latest),
    null,
  );
  return {
    now,
    activity,
    legacyRanking,
    pausedUntil: activity === null ? (account.data?.paused_until ?? null) : (activity?.paused_until ?? null),
    lastCall: activity === null ? lastCall : (activity?.last_call ?? null),
  };
}
