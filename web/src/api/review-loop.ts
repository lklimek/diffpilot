// Queries and mutations of the review loop (docs/review-workflow-plan.md §6: RC1–RC17, RW1–RW2, RN1–RN3) and the
// Ask questions about its items (RC7).
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, ApiError } from "./client";
import { withoutRetried } from "./queries";
import type {
  AnalysisId,
  AnchoredAskRequest,
  AskQuestion,
  ChannelKind,
  ContextQuery,
  DecideRequest,
  DecisionBatchRequest,
  DecisionId,
  Gate,
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

/** Every item with its assessment, decisions and effects (RC4); none without a PR or while not `enabled`. */
export const useCommentItems = (pr: PrRef | null, enabled = true) =>
  useQuery({
    queryKey: ["comment-items", ...(pr ? prKey(pr) : [])],
    queryFn: () => api.listCommentItems(pr!),
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
