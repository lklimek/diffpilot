// Queries and mutations of the review loop (docs/review-workflow-plan.md §6: RC1–RC17, RW1–RW2, RN1–RN3) and the
// Ask questions about its items (RC7).
import { useEffect, useRef, useState } from "react";
import {
  keepPreviousData,
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";
import { api, ApiError, AssessmentAdmissionError, errorText, type AssessmentAdmissionIssue } from "./client";
import { awaitsFinalAnswer, QUESTIONS_POLL_MS, withoutRetried } from "./questionHistory";
import type {
  CommentItem,
  CommentsOverview,
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

export const cachedCommentsOverview = (client: QueryClient, pr: PrRef) =>
  client.getQueryData<CommentsOverview>(overviewKey(pr));

/** One read of an analysis (the old progress URL finds its PR with it). */
export const useAnalysisOnce = (id: AnalysisId) =>
  useQuery({ queryKey: ["analysis-once", id], queryFn: () => api.getAnalysis(id) });

/** The PR page (RC1). */
export const usePrPage = (pr: PrRef) =>
  useQuery({ queryKey: ["pr-page", ...prKey(pr)], queryFn: () => api.getPrPage(pr) });

/** Comments overview: header, counts, rounds, queue (RC2); read again every minute. */
export const useCommentsOverview = (pr: PrRef, enabled = true) =>
  useQuery({
    queryKey: overviewKey(pr),
    queryFn: () => api.getComments(pr),
    enabled,
    refetchInterval: COMMENTS_POLL_MS,
  });

/** Every item with its assessment, decisions and effects (RC4); none without a PR or while not `enabled`. */
export const useCommentItems = (pr: PrRef | null, enabled = true) =>
  useQuery({
    queryKey: itemsKey(pr),
    queryFn: () => api.listCommentItems(pr!),
    refetchInterval: COMMENTS_POLL_MS,
    enabled: pr !== null && enabled,
  });

/** Standing GitHub imports since the visit preceding this one. */
export const useImportedCommentItems = (pr: PrRef, enabled: boolean) =>
  useQuery({
    queryKey: [...itemsKey(pr), "imported-unseen"],
    queryFn: () => api.listCommentItems(pr, { state: null, kind: null, imported_unseen: true }),
    refetchInterval: enabled ? COMMENTS_POLL_MS : false,
    enabled,
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

const MARKED_ITEMS_KEY = ["marked-items"] as const;

export function useMarkedItems() {
  return useInfiniteQuery({
    queryKey: MARKED_ITEMS_KEY,
    queryFn: ({ pageParam }) => api.listMarkedItems(pageParam),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor,
  });
}

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
  (error.status >= 500 && error.status !== 501) ||
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
    Promise.all([
      ...["pr-page", "comments", "comment-items", "comment-log", "decide-rest", "effects-preview"].map(
        (key) => client.invalidateQueries({ queryKey: [key, ...prKey(pr)] }),
      ),
      client.invalidateQueries({ queryKey: MARKED_ITEMS_KEY }),
    ]);
}

/** Records one entry so the import notice stays stable on later reads. */
export function useVisitComments(pr: PrRef) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: () => api.visitComments(pr),
    // A lost response can follow a committed write. Replaying it would advance the import cutoff.
    retry: false,
    onSuccess: () =>
      Promise.all([
        client.invalidateQueries({ queryKey: overviewKey(pr), exact: true }),
        client.invalidateQueries({ queryKey: [...itemsKey(pr), "imported-unseen"], exact: true }),
        client.invalidateQueries({ queryKey: ["dashboard-items"] }),
      ]),
  });
}

export type AssessmentStartIssue =
  AssessmentAdmissionIssue | { code: "failed"; message: string } | { code: "busy" };

const ASSESSMENT_CONFLICT_BACKOFF_MS = [2_000, 8_000];

async function startCommentAssessment(pr: PrRef, signal?: AbortSignal): Promise<AssessmentStartIssue | null> {
  for (let attempt = 0; !signal?.aborted; attempt += 1) {
    try {
      await api.startCommentAssessment(pr, signal);
      return null;
    } catch (error) {
      if (error instanceof AssessmentAdmissionError) return error.issue;
      if (!(error instanceof ApiError && error.status === 409 && error.code === "conflict")) {
        return { code: "failed", message: errorText(error, "Try again later.") };
      }
      if (!signal) return null;
    }
    const delay = ASSESSMENT_CONFLICT_BACKOFF_MS[attempt];
    if (delay === undefined) return { code: "busy" };
    await new Promise((resolve) => setTimeout(resolve, delay));
  }
  return null;
}

/** Start a pass on entry to Comments. */
export const useStartCommentAssessment = (pr: PrRef) =>
  useMutation({ mutationFn: () => startCommentAssessment(pr) });

/** Read GitHub again (RC3). A follow-up assessment has its own lifetime, separate from the read. */
export function useReadComments(pr: PrRef, onAssessment?: (issue: AssessmentStartIssue | null) => void) {
  const client = useQueryClient();
  const invalidate = useInvalidatePr(pr);
  const visit = useRef<{ controller: AbortController; assessing: boolean } | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    visit.current = { controller, assessing: false };
    return () => controller.abort();
  }, []);
  return useMutation({
    mutationFn: async () => {
      const before = onAssessment ? client.getQueryData<CommentItem[]>(itemsKey(pr)) : undefined;
      const known = before && new Set(before.map(({ no }) => no));
      await api.readComments(pr);
      await invalidate();
      const fresh = client.getQueryState<CommentItem[]>(itemsKey(pr));
      return known && fresh?.status === "success" && fresh.data?.some(({ no }) => !known.has(no));
    },
    onSuccess: (newItems) => {
      const current = visit.current;
      if (!newItems || !onAssessment || !current || current.assessing || current.controller.signal.aborted)
        return;
      current.assessing = true;
      onAssessment(null);
      void startCommentAssessment(pr, current.controller.signal).then((issue) => {
        current.assessing = false;
        if (current.controller.signal.aborted) return;
        onAssessment(issue);
        if (!issue) void invalidate();
      });
    },
  });
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

/** Skip every item of a question at its displayed revision, then refresh the reader's queue. */
export function useSkipCommentItems(pr: PrRef) {
  const invalidate = useInvalidatePr(pr);
  return useMutation({
    mutationFn: async (items: Pick<CommentItem, "no" | "revision">[]) => {
      const results = await Promise.allSettled(
        items.map(({ no, revision }) => api.skipCommentItem(pr, no, { item_revision: revision })),
      );
      const failure = results.find((result) => result.status === "rejected");
      if (failure) throw failure.reason;
    },
    onSettled: invalidate,
  });
}

/**
 * Undo of the reader's decision `decision` on item `C<no>` (RC12) at the item's current revision: the items are
 * read again first, so an Undo right after deciding never sends the revision from before that decision. When the
 * item's latest decision is no longer that one (another tab or reader decided since), nothing is sent.
 */
export type UndoTarget = { no: number; decision: DecisionId; mark?: boolean };

export function useUndoDecision(pr: PrRef) {
  const invalidate = useInvalidatePr(pr);
  const client = useQueryClient();
  return useMutation({
    mutationFn: async ({ no, decision, mark }: UndoTarget) => {
      const item = (await api.listCommentItems(pr)).find((i) => i.no === no);
      // The item exposes the active mark but its ID comes from the log. Read the revision before checking the ID.
      const matches = mark
        ? item?.not_a_review_point &&
          (await api.getCommentLog(pr)).marks?.some((entry) => entry.id === decision && !entry.undone_at)
        : !item?.not_a_review_point && item?.decisions.at(-1)?.id === decision;
      if (!item || !matches)
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

/** The reader's shared review history and standalone comments, read again while one is open. */
const prQuestionsKey = (pr: PrRef, analysisId: AnalysisId | null) => [
  "pr-questions",
  ...prKey(pr),
  analysisId,
];

export const usePrQuestions = (pr: PrRef, analysisId: AnalysisId | null | undefined) =>
  useQuery({
    queryKey: prQuestionsKey(pr, analysisId ?? null),
    queryFn: () => api.listPrQuestions(pr),
    enabled: analysisId !== undefined,
    // A failed question asked again shows once, as in the analysis chat.
    select: withoutRetried,
    refetchInterval: (query) => (query.state.data?.some(awaitsFinalAnswer) ? QUESTIONS_POLL_MS : false),
  });

/** Asks about a comment item (RC7) or continues an analysis thread from Comments. */
export function useAskPrQuestion(pr: PrRef, analysisId: AnalysisId | null) {
  const client = useQueryClient();
  const queryKey = prQuestionsKey(pr, analysisId);
  return useMutation({
    mutationFn: (body: AnchoredAskRequest) =>
      analysisId && body.anchor.kind !== "comment_item"
        ? api.askAnchored(analysisId, body)
        : api.askPrQuestion(pr, body),
    onSuccess: (question) => {
      client.setQueryData<AskQuestion[]>(queryKey, (all = []) => [
        ...all.filter((q) => q.id !== question.id),
        question,
      ]);
      void client.invalidateQueries({ queryKey });
    },
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
