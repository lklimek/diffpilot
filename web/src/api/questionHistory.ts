import type { AskQuestion, QuestionId } from "./generated/types";

/** Ordering of the first question in a thread, retained when it is retried. */
type FirstAsk = Pick<AskQuestion, "id" | "created_at">;

export type HistoryQuestion = AskQuestion & { firstAsk?: FirstAsk };

export const askOrder = (a: FirstAsk, b: FirstAsk): number =>
  Date.parse(a.created_at) - Date.parse(b.created_at) || a.id.localeCompare(b.id);

/** Polling interval for questions whose updates do not arrive on an analysis stream. */
export const QUESTIONS_POLL_MS = 3_000;

export const awaitsFinalAnswer = (q: AskQuestion) =>
  q.state === "pending" || q.state === "answering" || (q.state === "answered" && q.answered_at === null);

const key = (q: AskQuestion) => JSON.stringify([q.text, q.anchor, q.parent_id ?? null]);

/** A later ask of the same question replaces a failure unless it has follow-ups. */
export function withoutRetried(questions: readonly AskQuestion[]): HistoryQuestion[] {
  const parents = new Set(questions.flatMap((q) => (q.parent_id ? [q.parent_id] : [])));
  const ordered = [...questions].sort(askOrder);
  const isLaterAsk = (q: AskQuestion, next: AskQuestion) =>
    Date.parse(next.created_at) > Date.parse(q.created_at) && key(q) === key(next);
  const kept = ordered.filter(
    (q) => q.state !== "failed" || parents.has(q.id) || !ordered.some((next) => isLaterAsk(q, next)),
  );
  const ids = new Set(kept.map((q) => q.id));
  const firstAsks = new Map<QuestionId, FirstAsk>();
  for (const q of ordered) {
    if (q.parent_id || ids.has(q.id)) continue;
    const retry = kept.find((next) => isLaterAsk(q, next));
    if (retry && !firstAsks.has(retry.id)) firstAsks.set(retry.id, { id: q.id, created_at: q.created_at });
  }
  return questions
    .filter((q) => ids.has(q.id))
    .map((q) => {
      const firstAsk = firstAsks.get(q.id);
      return firstAsk ? { ...q, firstAsk } : q;
    });
}
