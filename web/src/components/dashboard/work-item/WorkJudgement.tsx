import type { DashboardItem } from "../../../api/provisional/dashboard";
import { itemJudgement } from "../../../api/provisional/dashboardJudgement";
import { dayText } from "../common/format";
import { JudgementMark } from "../judgement-mark/JudgementMark";
import type { Navigate } from "../common/navigate";

export interface WorkJudgementProps {
  onRejudge?: () => void;
  rejudging?: boolean;
  usageHref?: string;
}

export function judgementDate(item: DashboardItem) {
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
  item: DashboardItem;
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
