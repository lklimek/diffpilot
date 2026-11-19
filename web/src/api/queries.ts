// TanStack Query hooks over the API client; used by screens only (components get props).
import { QueryClient, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import type {
  AnalysisEvent,
  AnalysisId,
  AnalysisSummary,
  AskAnchor,
  AskQuestion,
  CreateAccessToken,
  CreateCommentRequest,
  DashboardSettings,
  Feedback,
  FeedbackId,
  FeedbackRequest,
  FilesResponse,
  FlowId,
  LlmLanguage,
  PendingReview,
  QuestionId,
  RangeQuery,
  RepoRef,
  StepId,
  Tour,
} from "./generated/types";
import { api } from "./client";
import { reviewStates } from "./review-state";

/** The signed-in user and CSRF token. */
export const useMe = () => useQuery({ queryKey: ["me"], queryFn: api.getMe });

/** The reader's own analyses, the latest per PR, newest first (Recent reviews); disabled while signed out. */
export const useMyAnalyses = (enabled: boolean) =>
  useQuery({ queryKey: ["my-analyses"], queryFn: () => api.listMyAnalyses(), enabled });

/** The review-ready analysis's tour, including pending flow placeholders. */
export const useTour = (id: AnalysisId) =>
  useQuery({ queryKey: ["tour", id], queryFn: () => api.getTour(id) });

/** The query of a range's lines, shared by `useLines` and one-off reads of the same cache entry. */
export const linesQuery = (id: AnalysisId, query: RangeQuery) => ({
  queryKey: ["lines", id, query],
  queryFn: () => api.getLines(id, query),
  staleTime: Infinity,
});

/** Lines of a range; disabled while `query` is undefined. */
export const useLines = (id: AnalysisId, query: RangeQuery | undefined) =>
  useQuery({ ...linesQuery(id, query!), enabled: query !== undefined });

/** Classic diff of a range; disabled while `query` is undefined. */
export const useDiff = (id: AnalysisId, query: RangeQuery | undefined) =>
  useQuery({
    queryKey: ["diff", id, query],
    queryFn: () => api.getDiff(id, query!),
    enabled: query !== undefined,
    staleTime: Infinity,
  });

// Questions: GET /questions, POST responses and `question` stream events write one cache entry. Arrival order
// decides (the contract allows answered → answering on a retry, so states have no ranking): a stream snapshot
// received after a GET started wins over that GET's copy; a POST response never replaces a snapshot.
const questionArrivals = new Map<AnalysisId, { seq: number; at: Map<QuestionId, number> }>();
const arrivals = (id: AnalysisId) => {
  let a = questionArrivals.get(id);
  if (!a) questionArrivals.set(id, (a = { seq: 0, at: new Map() }));
  return a;
};

/** The GET result reconciled with the cache: questions that arrived after `since` keep their cached copy. */
export function mergeFetchedQuestions(
  fetched: readonly AskQuestion[],
  cached: readonly AskQuestion[],
  arrivedAfter: (id: QuestionId) => boolean,
): AskQuestion[] {
  const newer = cached.filter((q) => arrivedAfter(q.id));
  return [
    ...fetched.map((q) => newer.find((n) => n.id === q.id) ?? q),
    ...newer.filter((n) => !fetched.some((q) => q.id === n.id)),
  ];
}

/** Questions asked about the analysis's steps, with their LLM answers. */
export const useQuestions = (id: AnalysisId) => {
  const client = useQueryClient();
  return useQuery({
    queryKey: ["questions", id],
    queryFn: async () => {
      const a = arrivals(id);
      const since = a.seq;
      const fetched = await api.listQuestions(id);
      const cached = client.getQueryData<AskQuestion[]>(["questions", id]) ?? [];
      return mergeFetchedQuestions(fetched, cached, (q) => (a.at.get(q) ?? -1) > since);
    },
  });
};

/** Records a question that arrived; `replace` false keeps an existing snapshot (POST responses). */
function arrive(client: QueryClient, id: AnalysisId, question: AskQuestion, replace: boolean): void {
  client.setQueryData<AskQuestion[]>(["questions", id], (all = []) => {
    if (!replace && all.some((q) => q.id === question.id)) return all;
    const a = arrivals(id);
    a.seq += 1;
    a.at.set(question.id, a.seq);
    return [...all.filter((q) => q.id !== question.id), question];
  });
}

/** Asks about a step; the pending question joins the list, its answer arrives on the event stream. */
export const useAskQuestion = (id: AnalysisId) => {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ step, text }: { step: StepId; text: string }) => api.askQuestion(id, step, { text }),
    onSuccess: (question) => arrive(client, id, question, false),
  });
};

/** Asks about an anchor (a step, lines of it, or lines or a file without a step), like `useAskQuestion`. */
export const useAskAnchored = (id: AnalysisId) => {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ anchor, text }: { anchor: AskAnchor; text: string }) =>
      api.askAnchored(id, { anchor, text }),
    onSuccess: (question) => arrive(client, id, question, false),
  });
};

/** The PR's files with their hunks and the user's GitHub Viewed marks; read again when the tab regains focus. */
export const useFiles = (id: AnalysisId) =>
  useQuery({ queryKey: ["files", id], queryFn: () => api.getFiles(id), refetchOnWindowFocus: true });

/**
 * Marks or unmarks a file as viewed on GitHub. Nothing changes before GitHub confirmed; then the cached mark is
 * set and the list read again.
 */
export const useSetFileViewed = (id: AnalysisId) => {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ path, viewed }: { path: string; viewed: boolean }) =>
      api.setFileViewed(id, { path, viewed }),
    onSuccess: (_, { path, viewed }) => {
      client.setQueryData<FilesResponse>(
        ["files", id],
        (files) =>
          files && {
            ...files,
            files: files.files.map((f) =>
              f.path === path ? { ...f, viewed: viewed ? "viewed" : "unviewed" } : f,
            ),
          },
      );
      return client.invalidateQueries({ queryKey: ["files", id] });
    },
  });
};

/**
 * Keeps the questions current from the analysis event stream while mounted. When the browser gives up on the
 * stream, the list is read again and the stream reopened (its snapshot resyncs); `lost` meanwhile.
 */
export function useQuestionEvents(id: AnalysisId): { lost: boolean } {
  const client = useQueryClient();
  const [lost, setLost] = useState(false);
  useEffect(
    () =>
      followAnalysisEvents(id, {
        onEvent: (event) => {
          setLost(false);
          if (
            event.event === "analysis" &&
            event.summary.id === id &&
            event.summary.review_ready !== undefined
          ) {
            // A flow accepted meanwhile changes who explains the lines of the Files view too.
            void client.invalidateQueries({ queryKey: ["tour", id] });
            void client.invalidateQueries({ queryKey: ["files", id] });
          }
          if (event.event === "question") arrive(client, id, event.question, true);
          if (event.event === "analysis" && event.summary.id === id && event.summary.usage !== undefined) {
            client.setQueryData<Tour>(
              ["tour", id],
              (tour) =>
                tour && {
                  ...tour,
                  analysis: {
                    ...tour.analysis,
                    usage: event.summary.usage,
                    models_used: event.summary.models_used ?? tour.analysis.models_used,
                  },
                },
            );
          }
        },
        // The stream ended and could not reopen: re-read the tour too, so a refusal (session over, analysis
        // gone) leads to its normal page instead of stale data.
        onClosed: () => {
          setLost(true);
          return client
            .refetchQueries({ queryKey: ["questions", id] })
            .then(() => client.refetchQueries({ queryKey: ["tour", id] }))
            .catch(() => {});
        },
      }),
    [client, id],
  );
  return { lost };
}

/** Interval of re-reading feedback while a MemCan write is pending. */
export const FEEDBACK_POLL_MS = 2000;

/** Interval while a write failed: the server retries it (30 s retry, 60 s sweep), so read again slowly. */
export const FEEDBACK_RETRY_POLL_MS = 15_000;

/** How often to re-read feedback: until every MemCan write is `written`; never while all are. */
function feedbackPollInterval(entries: readonly Feedback[] | undefined): number | false {
  if (entries?.some((f) => f.memory === "pending")) return FEEDBACK_POLL_MS;
  if (entries?.some((f) => f.memory === "failed")) return FEEDBACK_RETRY_POLL_MS;
  return false;
}

/** Stored feedback of the analysis (user values and `missed_by_llm`). */
export const useFeedback = (id: AnalysisId) =>
  useQuery({
    queryKey: ["feedback", id],
    queryFn: () => api.listFeedback(id),
    // MemCan writes run after the response and failed ones are retried by the server: read again until written.
    refetchInterval: (query) => feedbackPollInterval(query.state.data),
    // Paused while the tab is hidden; a returning tab reads again on focus.
    refetchIntervalInBackground: false,
  });

/** Stores a feedback value; the list is read again (the server merges values per target). */
export const usePostFeedback = (id: AnalysisId) => {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (request: FeedbackRequest) => api.postFeedback(id, request),
    onSuccess: () => client.invalidateQueries({ queryKey: ["feedback", id] }),
  });
};

/** Removes a stored feedback value; the list is read again. */
export const useDeleteFeedback = (id: AnalysisId) => {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (feedbackId: FeedbackId) => api.deleteFeedback(id, feedbackId),
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
  /** Browser time when the last event of the stream arrived (heartbeats included), or the analysis was read. */
  lastSignalAt?: number;
  /** Browser minus server clock (ms) from the last heartbeat (`at` vs. arrival); null before the first one. */
  clockOffset?: number | null;
}

/** A stream without any event (heartbeats come every 15 s) for this long is reopened. */
export const ANALYSIS_SILENCE_MS = 45_000;

/** What to do with an analysis event stream (`followAnalysisEvents`). */
export interface FollowHandlers {
  /** One event of the stream. */
  onEvent: (event: AnalysisEvent) => void;
  /** The connection dropped; the browser retries on its own. */
  onDrop?: () => void;
  /** The browser gave up: resync; resolving reopens the stream after `ANALYSIS_RECONNECT_MS`, rejecting stops. */
  onClosed: () => Promise<unknown>;
}

/**
 * Opens the analysis event stream and reopens it after the browser gave up; with `silenceMs`, also when no event
 * (heartbeats included) arrived for that long (an open but dead connection). Returns the function that stops.
 */
export function followAnalysisEvents(
  id: AnalysisId,
  handlers: FollowHandlers,
  silenceMs?: number,
): () => void {
  let active = true;
  let close = () => {};
  let timer: ReturnType<typeof setTimeout> | undefined;
  let watchdog: ReturnType<typeof setTimeout> | undefined;
  const watch = () => {
    if (silenceMs === undefined) return;
    clearTimeout(watchdog);
    watchdog = setTimeout(() => {
      if (!active) return;
      handlers.onDrop?.();
      close();
      connect();
    }, silenceMs);
  };
  const connect = () => {
    watch();
    close = api.subscribeAnalysis(id, {
      onEvent: (event) => {
        watch();
        handlers.onEvent(event);
      },
      onError: (closed) => {
        if (!closed) {
          handlers.onDrop?.();
          return;
        }
        close();
        handlers.onClosed().then(
          () => {
            if (active) timer = setTimeout(connect, ANALYSIS_RECONNECT_MS);
          },
          () => {},
        );
      },
    });
  };
  connect();
  return () => {
    active = false;
    clearTimeout(timer);
    clearTimeout(watchdog);
    close();
  };
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
    // State kept across an error only when it is this analysis's own.
    const own = (current: AnalysisLive & { id?: AnalysisId }): Partial<AnalysisLive> =>
      current.id === id ? current : {};
    const arrived = (summary: AnalysisSummary, reconnecting: boolean) => {
      const now = Date.now();
      setLive((current) => ({
        clockOffset: null,
        ...own(current),
        id,
        summary,
        reconnecting,
        lastSignalAt: now,
      }));
    };
    const stop = followAnalysisEvents(
      id,
      {
        onEvent: (event) => {
          const now = Date.now();
          if (event.event === "analysis") {
            if (event.summary.id === id) arrived(event.summary, false);
          } else if (event.event === "heartbeat") {
            const at = Date.parse(event.at);
            setLive((current) => ({
              clockOffset: null,
              ...own(current),
              id,
              reconnecting: false,
              lastSignalAt: now,
              ...(Number.isNaN(at) ? {} : { clockOffset: now - at }),
            }));
          } else {
            // Any other event (questions, or a type this client does not know) still shows the stream is alive.
            setLive((current) => ({ ...own(current), id, reconnecting: false, lastSignalAt: now }));
          }
        },
        onDrop: () => setLive((current) => ({ ...own(current), id, reconnecting: true })),
        onClosed: () =>
          api.getAnalysis(id).then(
            (summary) => {
              if (active) arrived(summary, true);
            },
            (error: unknown) => {
              if (active) setLive((current) => ({ ...own(current), id, error, reconnecting: false }));
              throw error;
            },
          ),
      },
      ANALYSIS_SILENCE_MS,
    );
    return () => {
      active = false;
      stop();
    };
  }, [id]);
  return live.id === id ? live : { reconnecting: false };
}

/** Save the shared installation model and refresh its audit details. */
export function useSetLlm() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: api.setLlm,
    onSuccess: (selection) => {
      client.setQueryData<import("./generated/types").Settings>(
        ["settings"],
        (old) => old && { ...old, llm_selection: selection },
      );
    },
  });
}

/** The user's dashboard settings (R6). */
export const useDashboardSettings = () =>
  useQuery({ queryKey: ["dashboard-settings"], queryFn: api.getDashboardSettings });

/** Saves the dashboard settings; the answer replaces the cached view. */
export function useSaveDashboardSettings() {
  const client = useQueryClient();
  return (settings: DashboardSettings) =>
    api.setDashboardSettings(settings).then((view) => {
      client.setQueryData(["dashboard-settings"], view);
      return view;
    });
}

/** Saves the installation-wide Ask effort and refreshes its audit details. */
export function useSetAskEffort() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: api.setAskEffort,
    onSuccess: (selection) =>
      client.setQueryData<import("./generated/types").Settings>(
        ["settings"],
        (old) => old && { ...old, llm_selection: selection },
      ),
  });
}

/** Saves the installation-wide ranking effort and refreshes its audit details. */
export function useSetJudgeEffort() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: api.setJudgeEffort,
    onSuccess: (selection) =>
      client.setQueryData<import("./generated/types").Settings>(
        ["settings"],
        (old) => old && { ...old, llm_selection: selection },
      ),
  });
}

/** The user's access tokens (R8). */
export const useAccessTokens = () => useQuery({ queryKey: ["access-tokens"], queryFn: api.listAccessTokens });

/**
 * Create and revoke access tokens. Plain calls, no mutation cache: the created secret goes only to the caller
 * (ADR-019) and the list is refetched.
 */
export function useAccessTokenActions() {
  const client = useQueryClient();
  const refetch = () => client.invalidateQueries({ queryKey: ["access-tokens"] });
  return {
    create: async (body: CreateAccessToken) => {
      const created = await api.createAccessToken(body);
      void refetch();
      return created;
    },
    revoke: async (id: string) => {
      await api.revokeAccessToken(id);
      await refetch();
    },
  };
}

/** `POST /auth/logout`; `everywhere` also ends the other sessions and revokes the personal access tokens. */
export const useSignOut = () =>
  useMutation({ mutationFn: (everywhere: boolean) => api.logout({ everywhere }) });

/** Pause future jobs; the current one finishes. */
export const usePauseAnalysis = () => useMutation({ mutationFn: api.pauseAnalysis });
/** Continue a user-paused analysis. */
export const useResumeAnalysis = () => useMutation({ mutationFn: api.resumeAnalysis });

/** Durable flow demand, background preference, resume and retry from the Tour, each refreshing the tour. */
/**
 * A flow analysed again loses its ticks (every user), questions and feedback on the server, and its new steps reuse
 * the step ids: forget its ticks here and read feedback and questions again.
 */
function forgetFlow(client: QueryClient, id: AnalysisId, flow: FlowId): void {
  reviewStates.forgetFlow(id, flow);
  void client.invalidateQueries({ queryKey: ["feedback", id] });
  void client.invalidateQueries({ queryKey: ["questions", id] });
}

/** Forgets the flows of `tour` that stop being analysed (analysed again, by anyone) while it is shown. */
export function useForgetReanalysedFlows(id: AnalysisId, tour: Tour | undefined): void {
  const client = useQueryClient();
  const analysed = useRef<ReadonlySet<FlowId> | null>(null);
  useEffect(() => {
    if (!tour) return;
    const now = new Set(
      tour.flows.filter((f) => !f.analysis_status || f.analysis_status.state === "analysed").map((f) => f.id),
    );
    const before = analysed.current;
    analysed.current = now;
    for (const flow of before ?? []) {
      if (!now.has(flow) && tour.flows.some((f) => f.id === flow)) forgetFlow(client, id, flow);
    }
  }, [client, id, tour]);
}

export function useFlowWork(id: AnalysisId) {
  const client = useQueryClient();
  const refresh = () =>
    Promise.all([
      client.invalidateQueries({ queryKey: ["tour", id] }),
      client.invalidateQueries({ queryKey: ["files", id] }),
    ]);
  return {
    request: useMutation({
      mutationFn: (flow: string) => api.requestFlow(id, flow),
      onSuccess: (_, flow) => {
        forgetFlow(client, id, flow);
        return refresh();
      },
    }),
    background: useMutation({
      mutationFn: (enabled: boolean) => api.setBackgroundAnalysis(id, enabled),
      onSuccess: refresh,
    }),
    resume: useMutation({ mutationFn: () => api.resumeAnalysis(id), onSuccess: refresh }),
    pause: useMutation({ mutationFn: () => api.pauseAnalysis(id), onSuccess: refresh }),
    retry: useMutation({ mutationFn: () => api.retryAnalysis(id), onSuccess: refresh }),
  };
}
