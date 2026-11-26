import { useQuery } from "@tanstack/react-query";
import { api } from "../../api/client";
import { useNow } from "../../app/now";
import { llmActivityKey } from "../../app/userEvents";

/** Bootstrap job activity, then follow the signed-in user's stream. */
export function useLlmActivity(login: string) {
  const now = new Date(useNow());
  const jobs = useQuery({
    queryKey: llmActivityKey(login),
    queryFn: ({ signal }) => api.getLlmActivity(signal),
    retry: false,
    staleTime: Infinity,
    refetchOnReconnect: false,
    refetchOnWindowFocus: false,
    refetchOnMount: false,
  });
  return { now, activity: jobs.data === undefined && jobs.isError ? null : jobs.data };
}
