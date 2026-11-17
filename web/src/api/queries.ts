// TanStack Query hooks over the API client; used by screens only (components get props).
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import type { AnalysisId, AnalysisSummary, LlmLanguage, RangeQuery, RepoRef } from "./generated/types";
import { api } from "./client";

/** The signed-in user and CSRF token. */
export const useMe = () => useQuery({ queryKey: ["me"], queryFn: api.getMe });

/** The finished analysis's tour. */
export const useTour = (id: AnalysisId) =>
  useQuery({ queryKey: ["tour", id], queryFn: () => api.getTour(id) });

/** The user's review state of an analysis. */
export const useReviewState = (id: AnalysisId) =>
  useQuery({ queryKey: ["review-state", id], queryFn: () => api.getReviewState(id) });

/** Lines of a range; disabled while `query` is undefined. */
export const useLines = (id: AnalysisId, query: RangeQuery | undefined) =>
  useQuery({
    queryKey: ["lines", id, query],
    queryFn: () => api.getLines(id, query!),
    enabled: query !== undefined,
    staleTime: Infinity,
  });

/** Resolves a PR URL: the PR now, its latest analysis, head moved, commenting available. */
export const useResolvePr = () => useMutation({ mutationFn: (url: string) => api.resolvePr({ url }) });

/** Requests a fresh analysis of a PR at the head the user saw. */
export const useCreateAnalysis = () => useMutation({ mutationFn: api.createAnalysis });

/** Cancels an analysis (partial results are discarded). */
export const useCancelAnalysis = () => useMutation({ mutationFn: api.cancelAnalysis });

/** Retries a failed analysis; progress arrives on the event stream. */
export const useRetryAnalysis = () => useMutation({ mutationFn: api.retryAnalysis });

/** Global settings: LLM status of the last call, language, App installations, known repositories. */
export const useSettings = () => useQuery({ queryKey: ["settings"], queryFn: api.getSettings });

/** Read-only settings of one repository; disabled while `repo` is undefined. */
export const useRepoSettings = (repo: RepoRef | undefined) =>
  useQuery({
    queryKey: ["repo-settings", repo?.owner, repo?.name],
    queryFn: () => api.getRepoSettings(repo!),
    enabled: repo !== undefined,
  });

/** Sets the language of LLM texts; refreshes the settings and the user. */
export const useSetLanguage = () => {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (language: LlmLanguage) => api.setLanguage({ language }),
    onSuccess: () =>
      Promise.all([
        client.invalidateQueries({ queryKey: ["settings"] }),
        client.invalidateQueries({ queryKey: ["me"] }),
      ]),
  });
};

/** Delay before a new event stream when the browser gave up on the previous one. */
export const ANALYSIS_RECONNECT_MS = 5000;

/** Live state of an analysis from its event stream (sse-events.md). */
export interface AnalysisLive {
  /** The latest snapshot; undefined until the first one arrives. */
  summary?: AnalysisSummary;
  /** Why the analysis cannot be shown (the stream failed and the analysis could not be read). */
  error?: unknown;
  /** The connection broke and is being re-established; `summary` may be stale until the next snapshot. */
  reconnecting: boolean;
}

/**
 * Follows an analysis over SSE: every (re)connect starts with a snapshot, each event replaces the summary.
 * The browser retries dropped connections itself; when it gives up (HTTP error) the analysis is read once:
 * an API error is reported, otherwise a new stream opens after `ANALYSIS_RECONNECT_MS`.
 */
export function useAnalysisLive(id: AnalysisId): AnalysisLive {
  // Tagged with its analysis so a changed `id` never shows the previous analysis's state.
  const [live, setLive] = useState<AnalysisLive & { id?: AnalysisId }>({ reconnecting: false });
  useEffect(() => {
    let active = true;
    let close = () => {};
    let timer: ReturnType<typeof setTimeout> | undefined;
    const connect = () => {
      close = api.subscribeAnalysis(id, {
        onEvent: (event) => {
          if (event.event === "analysis" && event.summary.id === id) {
            setLive({ id, summary: event.summary, reconnecting: false });
          }
        },
        onError: (closed) => {
          if (!closed) {
            setLive((current) => ({ ...current, id, reconnecting: true }));
            return;
          }
          close();
          api.getAnalysis(id).then(
            (summary) => {
              if (!active) return;
              setLive({ id, summary, reconnecting: true });
              timer = setTimeout(connect, ANALYSIS_RECONNECT_MS);
            },
            (error: unknown) => {
              if (active) setLive((current) => ({ ...current, id, error, reconnecting: false }));
            },
          );
        },
      });
    };
    connect();
    return () => {
      active = false;
      clearTimeout(timer);
      close();
    };
  }, [id]);
  return live.id === id ? live : { reconnecting: false };
}
