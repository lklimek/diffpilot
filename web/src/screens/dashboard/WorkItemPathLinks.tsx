import type { ReactNode } from "react";
import type { PrRef, ReviewLink, WorkItem } from "../../api/generated/types";
import { useFiles } from "../../api/queries";
import { FilePathLinks } from "../review/AnalysisPathLinks";

/** A ranking row can verify paths against its own PR's current analysis. */
export function WorkItemPathLinks({
  item,
  expanded,
  children,
}: {
  item: WorkItem;
  expanded: boolean;
  children: ReactNode;
}) {
  if (item.kind !== "pr" || item.private || !item.repo || item.number === null) return children;
  const review = item.llm && !item.review?.head_moved ? item.review : null;
  return (
    <RankedPrFiles pr={{ repo: item.repo, number: item.number }} review={review} expanded={expanded}>
      {children}
    </RankedPrFiles>
  );
}

function RankedPrFiles({
  pr,
  review,
  expanded,
  children,
}: {
  pr: PrRef;
  review: ReviewLink | null;
  expanded: boolean;
  children: ReactNode;
}) {
  // Collapsed rows can use an index already read in Files, without fanning out across the ranking.
  const id = review?.analysis_id ?? null;
  const { data } = useFiles(id ?? "", id !== null && expanded);
  const current = data?.current_head == null || data.current_head === review?.analysed_head;
  return (
    <FilePathLinks analysisId={id} pr={pr} files={current ? data?.files : undefined}>
      {children}
    </FilePathLinks>
  );
}
