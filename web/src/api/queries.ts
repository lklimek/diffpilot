// TanStack Query hooks over the API client; used by screens only (components get props).
import { useMutation, useQuery } from "@tanstack/react-query";
import type { AnalysisId, CreateAnalysisRequest, RangeQuery } from "./generated/types";
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

/** Resolve a PR URL, then reuse its latest analysis (head-moved choice: WP3 Home) or request one. */
export const useOpenPr = () =>
  useMutation({
    mutationFn: async (url: string) => {
      const resolved = await api.resolvePr({ url });
      if (resolved.analysis) return resolved.analysis;
      const request: CreateAnalysisRequest = {
        pr: { repo: resolved.pr.repo, number: resolved.pr.number },
        head_sha: resolved.pr.head_sha,
      };
      return api.createAnalysis(request);
    },
  });
