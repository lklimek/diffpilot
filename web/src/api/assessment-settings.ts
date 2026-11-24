import { useEffect } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "./client";
import type { PrRef } from "./generated/types";

/** Safe plan status; a read never initiates a device challenge. */
export function useCodexAccount(now: number) {
  const client = useQueryClient();
  const link = useMutation({ mutationFn: api.linkCodex, retry: false });
  const unlink = useMutation({
    mutationFn: api.unlinkCodex,
    onSuccess: async () => {
      link.reset();
      await Promise.all([
        client.invalidateQueries({ queryKey: ["codex"] }),
        client.invalidateQueries({ queryKey: ["settings"] }),
      ]);
    },
  });
  const status = useQuery({
    queryKey: ["codex", link.submittedAt, link.isSuccess],
    queryFn: async () => {
      if (link.isSuccess) return api.getCodexLink();
      const current = await api.getCodex();
      return current.state === "linking" ? api.getCodexLink() : current;
    },
    enabled: !link.isPending && !unlink.isPending,
    refetchInterval: (query) =>
      (!query.state.data || query.state.data.state === "linking") &&
      (!link.data || Date.parse(link.data.expires_at) > now)
        ? 2000
        : false,
    retry: false,
  });
  useEffect(() => {
    if (status.data?.state === "linked") void client.invalidateQueries({ queryKey: ["settings"] });
  }, [client, status.data]);
  const challenge =
    status.data?.state === "linked" || status.data?.state === "relink_needed" ? undefined : link.data;
  return { status, challenge, link, unlink };
}

/** User configuration plus the server's current UTC quota. */
export const useAssessmentSettings = () =>
  useQuery({
    queryKey: ["assessment-settings"],
    queryFn: api.getAssessmentSettings,
    refetchInterval: 30_000,
  });

/** Saving choices does not enqueue a pass. */
export function useSetAssessmentSettings() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: api.setAssessmentSettings,
    onSuccess: (data) => client.setQueryData(["assessment-settings"], data),
  });
}

/** Lazy, read-only report; polling observes method completion while expanded. */
export const useAssessmentComparison = (pr: PrRef, enabled: boolean) =>
  useQuery({
    queryKey: ["assessment-comparison", pr.repo.owner, pr.repo.name, pr.number],
    queryFn: () => api.getAssessmentComparison(pr),
    enabled,
    refetchInterval: (query) =>
      query.state.data?.runs.some((run) => run.status === "running" || run.status === "queued")
        ? 2000
        : false,
  });
