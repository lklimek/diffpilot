// Queries and mutations of the review loop (docs/review-workflow-plan.md §6: RC1–RC11, RW1–RW2, RN1–RN3).
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "./client";
import type {
  AnalysisId,
  ChannelKind,
  CommentItemId,
  ContextQuery,
  DecideRequest,
  DecisionBatchRequest,
  PrRef,
  PutChannelRequest,
} from "./generated/types";

/** How often Comments reads the overview and items while a page shows them (plan §4.1). */
export const COMMENTS_POLL_MS = 60_000;

const prKey = (pr: PrRef) => [pr.repo.owner, pr.repo.name, pr.number] as const;

/** One read of an analysis (the old progress URL finds its PR with it). */
export const useAnalysisOnce = (id: AnalysisId) =>
  useQuery({ queryKey: ["analysis-once", id], queryFn: () => api.getAnalysis(id) });

/** The PR page (RC1). */
export const usePrPage = (pr: PrRef) =>
  useQuery({ queryKey: ["pr-page", ...prKey(pr)], queryFn: () => api.getPrPage(pr) });

/** Comments overview: header, counts, rounds, queue (RC2); read again every minute. */
export const useCommentsOverview = (pr: PrRef) =>
  useQuery({
    queryKey: ["comments", ...prKey(pr)],
    queryFn: () => api.getComments(pr),
    refetchInterval: COMMENTS_POLL_MS,
  });

/** Every item with its assessment, decisions and effects (RC4). */
export const useCommentItems = (pr: PrRef) =>
  useQuery({
    queryKey: ["comment-items", ...prKey(pr)],
    queryFn: () => api.listCommentItems(pr),
    refetchInterval: COMMENTS_POLL_MS,
  });

/** The code an item discusses, its flows and notes elsewhere (RC5); none without an item. */
export const useCommentContext = (pr: PrRef, item: CommentItemId | null, query: ContextQuery) =>
  useQuery({
    queryKey: ["comment-context", ...prKey(pr), item, query.from, query.to],
    queryFn: () => api.getCommentContext(pr, item!, query),
    enabled: item !== null,
  });

/** Decision log and feed state (RC10). */
export const useCommentLog = (pr: PrRef) =>
  useQuery({ queryKey: ["comment-log", ...prKey(pr)], queryFn: () => api.getCommentLog(pr) });

/** Carry-overs of one PR (`owner/name/number`) or of all (RC11). */
export const useCarryOvers = (pr: string | null) =>
  useQuery({ queryKey: ["carry-overs", pr], queryFn: () => api.listCarryOvers({ pr, cursor: null }) });

/** The reader's watched PRs (RW2). */
export const useWatches = () => useQuery({ queryKey: ["watches"], queryFn: api.listWatches });

/** Everything that shows the review-loop state of `pr` is read again. */
function useInvalidatePr(pr: PrRef) {
  const client = useQueryClient();
  return () =>
    Promise.all(
      ["pr-page", "comments", "comment-items", "comment-log"].map((key) =>
        client.invalidateQueries({ queryKey: [key, ...prKey(pr)] }),
      ),
    );
}

/** Read GitHub again (RC3), then everything of the PR. */
export function useReadComments(pr: PrRef) {
  const invalidate = useInvalidatePr(pr);
  return useMutation({ mutationFn: () => api.readComments(pr), onSuccess: invalidate });
}

/** One decision (RC8). */
export function useDecide(pr: PrRef) {
  const invalidate = useInvalidatePr(pr);
  return useMutation({
    mutationFn: ({ item, body }: { item: CommentItemId; body: DecideRequest }) =>
      api.decideComment(pr, item, body),
    onSettled: invalidate,
  });
}

/** A group, "Decide the rest" or table batch (RC9). */
export function useDecideBatch(pr: PrRef) {
  const invalidate = useInvalidatePr(pr);
  return useMutation({
    mutationFn: (body: DecisionBatchRequest) => api.decideCommentBatch(pr, body),
    onSettled: invalidate,
  });
}

/** Watch or unwatch `pr` (RW1). */
export function useWatchPr(pr: PrRef) {
  const client = useQueryClient();
  const invalidate = useInvalidatePr(pr);
  return useMutation({
    mutationFn: (watch: boolean) => (watch ? api.watchPr(pr) : api.unwatchPr(pr)),
    onSettled: () => Promise.all([invalidate(), client.invalidateQueries({ queryKey: ["watches"] })]),
  });
}

/** Unwatch any PR from the watched list (RW1). */
export function useUnwatch() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (pr: PrRef) => api.unwatchPr(pr),
    onSettled: () => client.invalidateQueries({ queryKey: ["watches"] }),
  });
}

/** Notification channels (RN1). */
export const useNotificationSettings = () =>
  useQuery({ queryKey: ["notification-settings"], queryFn: api.getNotificationSettings });

/** Save, remove and test a channel (RN2, RN3). */
export function useChannelActions() {
  const client = useQueryClient();
  const refresh = () => client.invalidateQueries({ queryKey: ["notification-settings"] });
  return {
    save: useMutation({
      mutationFn: ({ kind, body }: { kind: ChannelKind; body: PutChannelRequest }) =>
        api.putNotificationChannel(kind, body),
      onSuccess: refresh,
    }),
    remove: useMutation({ mutationFn: api.deleteNotificationChannel, onSuccess: refresh }),
    test: useMutation({ mutationFn: api.testNotificationChannel, onSettled: refresh }),
  };
}
