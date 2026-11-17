// TanStack Query hooks over the API client; used by screens only (components get props).
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import type {
  AnalysisId,
  AnalysisSummary,
  AskQuestion,
  CreateCommentRequest,
  FeedbackRequest,
  LlmLanguage,
  PendingReview,
  RangeQuery,
  RepoRef,
  ReviewState,
  StepId,
} from "./generated/types";
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

/** Classic diff of a range; disabled while `query` is undefined. */
export const useDiff = (id: AnalysisId, query: RangeQuery | undefined) =>
  useQuery({
    queryKey: ["diff", id, query],
    queryFn: () => api.getDiff(id, query!),
    enabled: query !== undefined,
    staleTime: Infinity,
  });

/**
 * Changes the review state: the cache changes at once; with `persist` the state is also saved, one request at a
 * time, sending only the newest state waiting (latest wins). `error` is the last failure, cleared by a success.
 */
export function useSaveReviewState(id: AnalysisId, persist: boolean) {
  const client = useQueryClient();
  const queue = useRef<{ running: boolean; next: ReviewState | null }>({ running: false, next: null });
  const [error, setError] = useState<unknown>(null);
  const save = useCallback(
    (change: (state: ReviewState) => ReviewState) => {
      const key = ["review-state", id];
      const state = change(client.getQueryData<ReviewState>(key) ?? { ticked_steps: [], last_viewed: null });
      client.setQueryData(key, state);
      if (!persist) return;
      const q = queue.current;
      q.next = state;
      if (q.running) return;
      q.running = true;
      void (async () => {
        while (q.next) {
          const next = q.next;
          q.next = null;
          try {
            await api.putReviewState(id, next);
            setError(null);
          } catch (e) {
            setError(e);
          }
        }
        q.running = false;
      })();
    },
    [client, id, persist],
  );
  return { save, error };
}

/** Questions asked about the analysis's steps, with their LLM answers. */
export const useQuestions = (id: AnalysisId) =>
  useQuery({ queryKey: ["questions", id], queryFn: () => api.listQuestions(id) });

const upsertQuestion = (questions: AskQuestion[] | undefined, question: AskQuestion) => [
  ...(questions ?? []).filter((q) => q.id !== question.id),
  question,
];

/** Asks about a step; the pending question joins the list, its answer arrives on the event stream. */
export const useAskQuestion = (id: AnalysisId) => {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ step, text }: { step: StepId; text: string }) => api.askQuestion(id, step, { text }),
    onSuccess: (question) =>
      client.setQueryData<AskQuestion[]>(["questions", id], (all) => upsertQuestion(all, question)),
  });
};

/** Keeps the question list current from the analysis event stream while mounted. */
export function useQuestionEvents(id: AnalysisId): void {
  const client = useQueryClient();
  useEffect(
    () =>
      api.subscribeAnalysis(id, {
        onEvent: (event) => {
          if (event.event !== "question") return;
          client.setQueryData<AskQuestion[]>(["questions", id], (all) => upsertQuestion(all, event.question));
        },
        onError: () => {},
      }),
    [client, id],
  );
}

/** Stored feedback of the analysis (user values and `missed_by_llm`). */
export const useFeedback = (id: AnalysisId) =>
  useQuery({ queryKey: ["feedback", id], queryFn: () => api.listFeedback(id) });

/** Stores a feedback value; the list is read again (the server merges values per target). */
export const usePostFeedback = (id: AnalysisId) => {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (request: FeedbackRequest) => api.postFeedback(id, request),
    onSuccess: () => client.invalidateQueries({ queryKey: ["feedback", id] }),
  });
};

/** The user's GitHub pending review of the PR. */
export const usePendingReview = (id: AnalysisId) =>
  useQuery({ queryKey: ["pending-review", id], queryFn: () => api.getPendingReview(id) });

/** Comment writes into the pending review; each refreshes the review, a new comment also the feedback. */
export const useCommentActions = (id: AnalysisId) => {
  const client = useQueryClient();
  const refresh = () => client.invalidateQueries({ queryKey: ["pending-review", id] });
  const setComments = (change: (review: PendingReview) => PendingReview) =>
    client.setQueryData<PendingReview>(["pending-review", id], (review) => review && change(review));
  return {
    create: async (request: CreateCommentRequest) => {
      const response = await api.createComment(id, request);
      setComments((r) => ({
        ...r,
        comments: [...r.comments.filter((c) => c.id !== response.comment.id), response.comment],
      }));
      if (response.missed_by_llm_recorded) void client.invalidateQueries({ queryKey: ["feedback", id] });
      void refresh();
      return response;
    },
    edit: async (commentId: string, body: string) => {
      const comment = await api.updateComment(id, commentId, { body });
      setComments((r) => ({ ...r, comments: r.comments.map((c) => (c.id === comment.id ? comment : c)) }));
      void refresh();
    },
    remove: async (commentId: string) => {
      await api.deleteComment(id, commentId);
      setComments((r) => ({ ...r, comments: r.comments.filter((c) => c.id !== commentId) }));
      void refresh();
    },
  };
};

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
    // State kept across an error only when it is this analysis's own.
    const own = (current: AnalysisLive & { id?: AnalysisId }) => (current.id === id ? current : {});
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
            setLive((current) => ({ ...own(current), id, reconnecting: true }));
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
              if (active) setLive((current) => ({ ...own(current), id, error, reconnecting: false }));
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
