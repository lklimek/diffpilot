import type { WorkItem } from "../../../api/generated/types";
import { itemJudgement } from "../common/itemJudgement";
import { dayText } from "../common/format";
import { JudgementMark } from "../judgement-mark/JudgementMark";
import type { Navigate } from "../common/navigate";

export interface WorkJudgementProps {
  onRejudge?: () => void;
  rejudging?: boolean;
  usageHref?: string;
}

export function judgementDate(item: WorkItem) {
  const date = itemJudgement(item).judged_at;
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
  const judgement = itemJudgement(item);
  return (
    <span aria-live="polite">
      <JudgementMark
        judgement={rejudging ? { ...judgement, state: "judging" } : judgement}
        onRejudge={item.judgement ? onRejudge : undefined}
        usageHref={usageHref}
        onNavigate={onNavigate}
      />
    </span>
  );
}
