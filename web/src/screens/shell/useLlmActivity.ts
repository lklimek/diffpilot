import { useQuery } from "@tanstack/react-query";
import { api } from "../../api/client";
import type { LlmCallOutcome } from "../../api/generated/types";
import { fetchLlmActivity } from "../../api/provisional/llmActivity";
import { useNow } from "../../app/now";

const POLL_MS = 30_000;

/** Phase A polls job activity and reads status independently while the provisional endpoint is unavailable. */
export function useLlmActivity(login: string | null) {
  const now = new Date(useNow());
  const enabled = login !== null;
  const jobs = useQuery({
    queryKey: ["llm-activity", login],
    queryFn: ({ signal }) => fetchLlmActivity(signal),
    enabled,
    retry: false,
    refetchInterval: (query) => (query.state.data === null ? false : POLL_MS),
  });
  const settings = useQuery({
    queryKey: ["settings"],
    queryFn: api.getSettings,
    enabled,
    refetchInterval: POLL_MS,
  });
  const account = useQuery({
    queryKey: ["codex", "shell", login],
    queryFn: api.getCodex,
    enabled,
    refetchInterval: POLL_MS,
    retry: false,
  });
  const activity = jobs.isError ? null : jobs.data;
  const lastCall = [
    activity?.last_call,
    account.data?.last_call,
    settings.data?.llm_status,
  ].reduce<LlmCallOutcome | null>(
    (latest, call) => (call && (!latest || Date.parse(call.at) > Date.parse(latest.at)) ? call : latest),
    null,
  );
  return {
    now,
    activity,
    pausedUntil: activity ? activity.paused_until : (account.data?.paused_until ?? null),
    lastCall,
  };
}
