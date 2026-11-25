import { useQuery } from "@tanstack/react-query";
import { api } from "../../api/client";
import type { LlmCallOutcome } from "../../api/generated/types";
import { fetchLlmActivity } from "../../api/provisional/llmActivity";
import { useNow } from "../../app/now";

const POLL_MS = 30_000;

/** Phase A polls job activity and reads status independently while the provisional endpoint is unavailable. */
export function useLlmActivity(login: string, menuOpen: boolean) {
  const now = new Date(useNow());
  const jobs = useQuery({
    queryKey: ["llm-activity", login],
    queryFn: ({ signal }) => fetchLlmActivity(signal),
    retry: false,
    refetchInterval: (query) => (query.state.data === null ? false : POLL_MS),
  });
  const activity = jobs.isError ? null : jobs.data;
  const fallback = {
    enabled: menuOpen && activity === null,
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  };
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
    pausedUntil: activity === null ? (account.data?.paused_until ?? null) : (activity?.paused_until ?? null),
    lastCall: activity === null ? lastCall : (activity?.last_call ?? null),
  };
}
