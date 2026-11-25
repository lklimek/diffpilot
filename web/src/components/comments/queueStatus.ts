import type { CommentItem, NotAssessedReason, QueueEntry } from "../../api/generated/types";

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

export function commentStatus(item: CommentItem): QueueStatus {
  if (item.status_comment) return "status_comments";
  if (item.imported_decision || item.state === "outside") return "decided";
  if (followsPrimary(item)) return "duplicates";
  if (["decided", "executing", "done", "repeat"].includes(item.state)) return "decided";
  if (item.gone_at) return "all";
  // A linked item can lack its own assessment and still require an explicit decision.
  if (
    duplicatePrimary(item) != null &&
    (awaitsDecision(item) || (item.gates.length > 0 && item.state === "assessing"))
  )
    return "needs_you";
  if (item.assessment_state.state === "queued") return "waiting";
  if (item.assessment_state.state === "running") return "assessing";
  if (item.kind !== "pr_question" && item.assessment_state.state === "not_assessed") return "not_assessed";
  return awaitsDecision(item) ? "needs_you" : "all";
}

export function needsYouQueue(queue: QueueEntry[], items: ReadonlyMap<number, CommentItem>): QueueEntry[] {
  return queue.flatMap((entry) => {
    const item_nos = entry.item_nos.filter((no) => {
      const item = items.get(no);
      return item && commentStatus(item) === "needs_you";
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

/** Use the same queue membership as the list, including PR-level and individually decided gate items. */
export function needsYouCount(
  fallback: number,
  queue?: QueueEntry[],
  items?: ReadonlyMap<number, CommentItem>,
): number {
  if (!queue || !items) return fallback;
  return needsYouQueue(queue, items).reduce((count, entry) => count + entry.item_nos.length, 0);
}

export const commentsByNo = (items?: readonly CommentItem[]) =>
  items ? new Map(items.map((item) => [item.no, item])) : undefined;
