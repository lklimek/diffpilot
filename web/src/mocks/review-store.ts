// The reviewer's data as the mocks' server holds it: review state, questions, feedback and the pending review start
// from the fixture state and change with the Tour's requests. Kept in sessionStorage when there is one, so a page
// reload in the browser keeps it (Playwright "reload keeps position").
import type {
  AskQuestion,
  CreateCommentResponse,
  Feedback,
  PendingReview,
  ReviewState,
} from "../api/generated/types";
import { fixtureState, seedNow } from "./fixtures";
import { scenario } from "./scenarios";

/** Delay before a mocked question is answered. */
export const ANSWER_MS = 300;

/** How long a feedback entry stays `pending` after the mock first listed it, before its MemCan outcome. */
export const MEMCAN_SYNC_MS = 1500;

interface Store {
  reviewState: ReviewState;
  questions: AskQuestion[];
  feedback: Feedback[];
  pendingReview: PendingReview;
  /** When the mock first listed each pending feedback entry (ms since epoch). */
  memorySeenAt: Record<string, number>;
}

type QuestionListener = (question: AskQuestion) => void;

let memory: { state: string; store: Store } | null = null;
/** First response per comment idempotency key. */
export const commentResponses = new Map<string, CreateCommentResponse>();
const listeners = new Set<QuestionListener>();
const storageKey = () => `smart-review.mock.${fixtureState()}`;
const storage = (): Storage | undefined => globalThis.sessionStorage;

function seed(): Store {
  return {
    reviewState: seedNow("review-state") as ReviewState,
    questions: (scenario().questions ?? ((q: AskQuestion[]) => q))(seedNow("questions") as AskQuestion[]),
    feedback: seedNow("feedback") as Feedback[],
    pendingReview: (scenario().pendingReview ?? ((r: PendingReview) => r))(
      seedNow("pending-review") as PendingReview,
    ),
    memorySeenAt: {},
  };
}

/** The store of the selected fixture state (a copy is returned by the readers below). */
function store(): Store {
  if (memory?.state !== fixtureState()) {
    const saved = storage()?.getItem(storageKey());
    memory = { state: fixtureState(), store: saved ? (JSON.parse(saved) as Store) : seed() };
  }
  return memory.store;
}

/** Applies `change` to the store and saves it. */
export function updateStore(change: (store: Store) => void): void {
  const current = store();
  change(current);
  storage()?.setItem(storageKey(), JSON.stringify(current));
}

/** A copy of one part of the store. */
export function readStore<K extends keyof Store>(key: K): Store[K] {
  return structuredClone(store()[key]);
}

/** Publishes a question change to the open event streams. */
export function publishQuestion(question: AskQuestion): void {
  for (const listener of listeners) listener(structuredClone(question));
}

/** Calls `listener` on every question change until the returned function is called. */
export function subscribeQuestions(listener: QuestionListener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Back to the seed of every state; no listeners (tests). */
export function resetStore(): void {
  memory = null;
  listeners.clear();
  commentResponses.clear();
  const s = storage();
  if (!s) return;
  for (const key of Object.keys(s)) if (key.startsWith("smart-review.mock.")) s.removeItem(key);
}
