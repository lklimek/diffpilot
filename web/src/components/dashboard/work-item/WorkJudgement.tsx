import type { WorkItem } from "../../../api/generated/types";
import { dayText } from "../common/format";
import { JudgementMark } from "../judgement-mark/JudgementMark";
import type { Navigate } from "../common/navigate";

export interface WorkJudgementProps {
  onRejudge?: () => void;
  rejudging?: boolean;
  usageHref?: string;
}

export function judgementDate(item: WorkItem) {
  const date = item.judgement ? item.judgement.judged_at : item.llm?.judged_at;
  return date ? dayText(date) : null;
}

export function WorkJudgement({
  item,
  rejudging,
  onRejudge,
  usageHref,
  onNavigate,
}: WorkJudgementProps & {
  item: WorkItem;
  onNavigate?: Navigate;
}) {
  const { judgement } = item;
  if (!judgement) return null;
  return (
    <span aria-live="polite">
      <JudgementMark
        judgement={rejudging ? { ...judgement, state: "judging" } : judgement}
        onRejudge={onRejudge}
        usageHref={usageHref}
        onNavigate={onNavigate}
      />
    </span>
  );
}
