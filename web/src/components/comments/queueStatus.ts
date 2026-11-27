import type { CommentItem, CommentStatus, NotAssessedReason, QueueEntry } from "../../api/generated/types";

export const QUEUE_STATUS = {
  needs_you: "Needs you",
  assessing: "Assessing now",
  waiting: "Waiting for assessment",
  not_assessed: "Not assessed",
  decided: "Decided",
  not_a_review_point: "Not a review point",
  duplicates: "Duplicates",
  status_comments: "Status comments",
  all: "All",
} satisfies Record<CommentStatus, string>;

export type QueueStatus = CommentStatus;

export function queueStatusLabel(status: QueueStatus, inPass?: number): string {
  return status === "assessing" && inPass === undefined ? "Pending or assessing" : QUEUE_STATUS[status];
}

export const queueStatusOf = (value: string | null): QueueStatus =>
  value && Object.hasOwn(QUEUE_STATUS, value) ? (value as QueueStatus) : "needs_you";

export const NOT_ASSESSED: Record<NotAssessedReason, string> = {
  daily_limit: "daily limit",
  failed: "the assessment failed",
  missing: "left out of the pass",
  not_run: "no assessment has run for it yet",
};

export function duplicatePrimary(item: CommentItem) {
  return item.duplicate_of === item.no ? undefined : item.duplicate_of;
}

export const followsPrimary = (item: CommentItem) =>
  duplicatePrimary(item) != null && item.decision_follows_primary === true && item.gates.length === 0;

export const awaitsDecision = (item: CommentItem) =>
  !item.gone_at && ["undecided", "reopened", "asked_back"].includes(item.state);

export function needsYouQueue(queue: QueueEntry[], items: ReadonlyMap<number, CommentItem>): QueueEntry[] {
  return queue.flatMap((entry) => {
    const item_nos = entry.item_nos.filter((no) => {
      const item = items.get(no);
      return item?.status === "needs_you";
    });
    if (item_nos.length <= 1) {
      if (item_nos.length === 0) return [];
      return item_nos.length === entry.item_nos.length ? [entry] : [{ ...entry, item_nos, reason: "single" }];
    }
    const independent = item_nos.filter(
      (no) => duplicatePrimary(items.get(no)!) != null || items.get(no)!.gates.length > 0,
    );
    const grouped = item_nos.filter((no) => !independent.includes(no));
    return [
      ...independent.map((no): QueueEntry => ({
        ...entry,
        item_nos: [no],
        reason: items.get(no)!.gates.length ? "gate" : "single",
      })),
      ...(grouped.length
        ? [{ ...entry, item_nos: grouped, reason: grouped.length === 1 ? ("single" as const) : entry.reason }]
        : []),
    ];
  });
}

export const commentsByNo = (items?: readonly CommentItem[]) =>
  items ? new Map(items.map((item) => [item.no, item])) : undefined;
