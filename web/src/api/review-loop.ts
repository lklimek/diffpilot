// Queries and mutations of the review loop (docs/review-workflow-plan.md §6: RC1–RC17, RW1–RW2, RN1–RN3) and the
// Ask questions about its items (RC7).
import { useEffect, useState } from "react";
import {
  keepPreviousData,
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";
import { api, ApiError } from "./client";
import {
  fetchReviewCommentItems,
  fetchReviewComments,
  fetchReviewPrPage,
  type ReviewComment,
  type ReviewCommentsOverview,
} from "./provisional/comments";
import { withoutRetried } from "./questionHistory";
import type {
  AnalysisId,
  AnchoredAskRequest,
  AskQuestion,
  ChannelKind,
  ContextQuery,
  CarryOverPage,
  DecideRequest,
  DecisionBatchRequest,
  DecisionBatchRow,
  DecisionId,
  Gate,
  PrRef,
  PutChannelRequest,
} from "./generated/types";

/** How often Comments reads the overview and items while a page shows them (plan §4.1). */
export const COMMENTS_POLL_MS = 60_000;

const prKey = (pr: PrRef) => [pr.repo.owner, pr.repo.name, pr.number] as const;

const overviewKey = (pr: PrRef) => ["comments", ...prKey(pr)] as const;
const itemsKey = (pr: PrRef | null) => ["comment-items", ...(pr ? prKey(pr) : [])] as const;

export function cachedCommentsReview(client: QueryClient, pr: PrRef) {
  return {
    overview: client.getQueryData<ReviewCommentsOverview>(overviewKey(pr)),
    items: client.getQueryData<ReviewComment[]>(itemsKey(pr)),
  };
}

/** One read of an analysis (the old progress URL finds its PR with it). */
export const useAnalysisOnce = (id: AnalysisId) =>
  useQuery({ queryKey: ["analysis-once", id], queryFn: () => api.getAnalysis(id) });

/** The PR page (RC1). */
export const usePrPage = (pr: PrRef) =>
  useQuery({ queryKey: ["pr-page", ...prKey(pr)], queryFn: () => fetchReviewPrPage(pr) });

/** Comments overview: header, counts, rounds, queue (RC2); read again every minute. */
export const useCommentsOverview = (pr: PrRef, enabled = true) =>
  useQuery({
    queryKey: overviewKey(pr),
    queryFn: () => fetchReviewComments(pr),
    enabled,
    refetchInterval: COMMENTS_POLL_MS,
  });

/** Every item with its assessment, decisions and effects (RC4); none without a PR or while not `enabled`. */
export const useCommentItems = (pr: PrRef | null, enabled = true) =>
  useQuery({
    queryKey: itemsKey(pr),
    queryFn: () => fetchReviewCommentItems(pr!),
    refetchInterval: COMMENTS_POLL_MS,
    enabled: pr !== null && enabled,
  });

/** The code an item (`C<no>`) discusses, its flows and notes elsewhere (RC5); none without an item. */
export const useCommentContext = (pr: PrRef, no: number | null, query: ContextQuery) =>
  useQuery({
    queryKey: ["comment-context", ...prKey(pr), no, query.from, query.to],
    queryFn: () => api.getCommentContext(pr, no!, query),
    enabled: no !== null,
  });

/** Decision log and feed state (RC10). */
export const useCommentLog = (pr: PrRef) =>
  useQuery({ queryKey: ["comment-log", ...prKey(pr)], queryFn: () => api.getCommentLog(pr) });

/** Carry-overs of one PR (`owner/name/number`) or of all (RC11): the newest page, then older ones on request. */
export function useCarryOvers(pr: string | null) {
  const query = useInfiniteQuery({
    queryKey: ["carry-overs", pr],
    queryFn: ({ pageParam }) => api.listCarryOvers({ pr, cursor: pageParam }),
    initialPageParam: null as string | null,
    getNextPageParam: (last: CarryOverPage) => last.next_cursor,
  });
  const pages = query.data?.pages;
  // Counts come with every page; the events of the pages read so far are listed in order.
  const data: CarryOverPage | undefined = pages && {
    ...pages[0]!,
    events: pages.flatMap((page) => page.events),
    next_cursor: pages.at(-1)!.next_cursor,
  };
  return { ...query, data };
}

/** How long a draft waits for more typing before its effects are planned (RC16). */
export const PREVIEW_DEBOUNCE_MS = 250;

/** A preview failure worth trying again: no answer (status 0), a server failure (5xx), 408 or 429; other 4xx are
 * the input's. */
export const transientFailure = (error: unknown) =>
  !(error instanceof ApiError) ||
  error.status === 0 ||
  error.status >= 500 ||
  error.status === 408 ||
  error.status === 429;

/** The wait before try `attempt + 1` of a failed preview: doubling from 1 s, at most 30 s. */
export const previewRetryDelay = (attempt: number) => Math.min(1000 * 2 ** attempt, 30_000);

/**
 * The effects the server plans for `rows` (RC16), the reply texts it would post included; read again shortly after
 * the rows stop changing, the previous answer kept meanwhile. Each answer names the rows it was planned for
 * (`data.rows`, their JSON); `current` is true once the answer is for `rows` and nothing is being planned. `inputs`
 * is what else the plan depends on (a PR-level question: the PR's items); when it changes, the rows are planned
 * again. Nothing without rows. `planAgain` plans at once, also while a failed try waits for the next one.
 */
export function useEffectsPreview(pr: PrRef, rows: DecisionBatchRow[] | null, inputs: string | null = null) {
  const client = useQueryClient();
  const draft = JSON.stringify(rows);
  const [settled, setSettled] = useState(draft);
  useEffect(() => {
    const timer = setTimeout(() => setSettled(draft), PREVIEW_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [draft]);
  const planned = JSON.parse(settled) as DecisionBatchRow[] | null;
  const queryKey = ["effects-preview", ...prKey(pr), settled, inputs];
  const query = useQuery({
    queryKey,
    queryFn: async () => ({ rows: settled, items: await api.previewCommentEffects(pr, { rows: planned! }) }),
    enabled: planned !== null && planned.length > 0,
    placeholderData: keepPreviousData,
    // A row's plan depends only on its item (its revision is in the row); a PR-level plan also on other items.
    staleTime: inputs === null ? Infinity : 0,
    // Transient failures are tried again for as long as the card is open (no observer, no retry); a refusal is not.
    retry: (_, error) => transientFailure(error),
    retryDelay: previewRetryDelay,
  });
  // A refetch would wait out the backoff of a try still pending; the pending try is dropped instead.
  const planAgain = async () => {
    await client.cancelQueries({ queryKey, exact: true });
    await query.refetch();
  };
  return {
    ...query,
    current: query.data?.rows === draft && !query.isPlaceholderData && !query.isFetching,
    planAgain,
  };
}

/** The server's "Decide the rest per recommendation" list (RC17); read again with the items. */
export const useDecideRest = (pr: PrRef) =>
  useQuery({
    queryKey: ["decide-rest", ...prKey(pr)],
    queryFn: () => api.getDecideRest(pr),
    refetchInterval: COMMENTS_POLL_MS,
  });

/** The reader's watched PRs (RW2). */
export const useWatches = () => useQuery({ queryKey: ["watches"], queryFn: api.listWatches });

/** Everything that shows the review-loop state of `pr` is read again. */
function useInvalidatePr(pr: PrRef) {
  const client = useQueryClient();
  return () =>
    Promise.all(
      ["pr-page", "comments", "comment-items", "comment-log", "decide-rest", "effects-preview"].map((key) =>
        client.invalidateQueries({ queryKey: [key, ...prKey(pr)] }),
      ),
    );
}

/** Read GitHub again (RC3), then everything of the PR. */
export function useReadComments(pr: PrRef) {
  const invalidate = useInvalidatePr(pr);
  return useMutation({ mutationFn: () => api.readComments(pr), onSuccess: invalidate });
}

/** One decision on item `C<no>` (RC8). */
export function useDecide(pr: PrRef) {
  const invalidate = useInvalidatePr(pr);
  return useMutation({
    mutationFn: ({ no, body }: { no: number; body: DecideRequest }) => api.decideComment(pr, no, body),
    onSettled: invalidate,
  });
}

/** An owner action on item `C<no>` at the revision the reader saw: Undo (RC12), Reopen (RC13), Skip for now (RC14). */
export type ItemAction = "undo" | "reopen" | "skip";

/** Undo, Reopen or Skip for now, then everything of the PR is read again. */
export function useItemAction(pr: PrRef) {
  const invalidate = useInvalidatePr(pr);
  const client = useQueryClient();
  return useMutation({
    mutationFn: async ({ action, no, revision }: { action: ItemAction; no: number; revision: number }) => {
      const body = { item_revision: revision };
      if (action === "undo") await api.undoCommentDecision(pr, no, body);
      else if (action === "reopen") await api.reopenCommentItem(pr, no, body);
      else await api.skipCommentItem(pr, no, body);
    },
    onSettled: () => Promise.all([invalidate(), client.invalidateQueries({ queryKey: ["carry-overs"] })]),
  });
}

/**
 * Undo of the reader's decision `decision` on item `C<no>` (RC12) at the item's current revision: the items are
 * read again first, so an Undo right after deciding never sends the revision from before that decision. When the
 * item's latest decision is no longer that one (another tab or reader decided since), nothing is sent.
 */
export function useUndoDecision(pr: PrRef) {
  const invalidate = useInvalidatePr(pr);
  const client = useQueryClient();
  return useMutation({
    mutationFn: async ({ no, decision }: { no: number; decision: DecisionId }) => {
      const item = (await api.listCommentItems(pr)).find((i) => i.no === no);
      if (!item || item.decisions.at(-1)?.id !== decision)
        throw new ApiError(409, "item_changed", `C${no} changed since your decision. Read it again.`);
      return api.undoCommentDecision(pr, no, { item_revision: item.revision });
    },
    onSettled: () => Promise.all([invalidate(), client.invalidateQueries({ queryKey: ["carry-overs"] })]),
  });
}

/** "Assess again" of item `C<no>` (RC6): 202 when asked; 409 `conflict` while a pass of the PR is queued or running. */
export function useAssessAgain(pr: PrRef) {
  const invalidate = useInvalidatePr(pr);
  return useMutation({ mutationFn: (no: number) => api.assessComment(pr, no), onSettled: invalidate });
}

/** The owner marks or clears a gate on item `C<no>` (RC15). */
export function useMarkGate(pr: PrRef) {
  const invalidate = useInvalidatePr(pr);
  return useMutation({
    mutationFn: ({
      no,
      gate,
      marked,
      revision,
    }: {
      no: number;
      gate: Gate;
      marked: boolean;
      revision: number;
    }) => api.markCommentGate(pr, no, { gate, marked, item_revision: revision }),
    onSettled: invalidate,
  });
}

/** How often the reader's open questions about comment items are read again (no event stream without an analysis). */
export const PR_QUESTIONS_POLL_MS = 3_000;

const openQuestion = (q: AskQuestion) => q.state === "pending" || q.state === "answering";

/** The reader's questions about the PR's comment items (RC7), read again while one is open. */
export const usePrQuestions = (pr: PrRef) =>
  useQuery({
    queryKey: ["pr-questions", ...prKey(pr)],
    queryFn: () => api.listPrQuestions(pr),
    // A failed question asked again shows once, as in the analysis chat.
    select: withoutRetried,
    refetchInterval: (query) => (query.state.data?.some(openQuestion) ? PR_QUESTIONS_POLL_MS : false),
  });

/** Asks about a comment item (RC7); the question joins the list at once. */
export function useAskPrQuestion(pr: PrRef) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (body: AnchoredAskRequest) => api.askPrQuestion(pr, body),
    onSuccess: (question) =>
      client.setQueryData<AskQuestion[]>(["pr-questions", ...prKey(pr)], (all = []) => [
        ...all.filter((q) => q.id !== question.id),
        question,
      ]),
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
    onSettled: () =>
      Promise.all([
        invalidate(),
        client.invalidateQueries({ queryKey: ["watches"] }),
        client.invalidateQueries({ queryKey: ["pr-history"] }),
      ]),
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
