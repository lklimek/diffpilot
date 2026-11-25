import type { QueueEntry } from "../../api/generated/types";
import type { ReviewComment } from "../../api/provisional/comments";

export const QUEUE_STATUS = {
  needs_you: "Needs you",
  assessing: "Assessing now",
  waiting: "Waiting for assessment",
  not_assessed: "Not assessed",
  decided: "Decided",
  duplicates: "Duplicates",
  status_comments: "Status comments",
  all: "All",
} as const;

export type QueueStatus = keyof typeof QUEUE_STATUS;

export const queueStatusOf = (value: string | null): QueueStatus =>
  value && Object.hasOwn(QUEUE_STATUS, value) ? (value as QueueStatus) : "needs_you";

export const isDuplicate = (item: ReviewComment) => item.duplicate_of != null;

export function commentStatus(item: ReviewComment): QueueStatus {
  if (item.status_comment) return "status_comments";
  if (item.imported_decision || item.state === "outside") return "decided";
  if (isDuplicate(item)) return "duplicates";
  if (["decided", "executing", "done", "repeat"].includes(item.state)) return "decided";
  if (item.gone_at) return "all";
  if (item.assessment_queued) return "waiting";
  if (item.assessment_state.state === "running") return "assessing";
  if (item.kind !== "pr_question" && item.assessment_state.state === "not_assessed") return "not_assessed";
  return ["undecided", "reopened", "asked_back"].includes(item.state) ? "needs_you" : "all";
}

export function needsYouQueue(queue: QueueEntry[], items: ReadonlyMap<number, ReviewComment>): QueueEntry[] {
  return queue.flatMap((entry) => {
    const item_nos = entry.item_nos.filter((no) => {
      const item = items.get(no);
      return item && commentStatus(item) === "needs_you";
    });
    if (!item_nos.length) return [];
    if (item_nos.length === entry.item_nos.length) return [entry];
    return [{ ...entry, item_nos, reason: item_nos.length === 1 ? "single" : entry.reason }];
  });
}
