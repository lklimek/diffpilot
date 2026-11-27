import type { ReactNode } from "react";
import type { PrRef, ReviewLink, WorkItem } from "../../api/generated/types";
import { useFiles } from "../../api/queries";
import { hasFileMentionSyntax } from "../../components/section/pathLinks";
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
  const { llm, review } = item;
  if (
    item.kind !== "pr" ||
    item.private ||
    !item.repo ||
    item.number === null ||
    !llm ||
    !review ||
    review.head_moved
  )
    return children;
  const readIndex = expanded || [llm.why, llm.next, llm.alarm ?? ""].some(hasFileMentionSyntax);
  return (
    <RankedPrFiles pr={{ repo: item.repo, number: item.number }} review={review} readIndex={readIndex}>
      {children}
    </RankedPrFiles>
  );
}

function RankedPrFiles({
  pr,
  review,
  readIndex,
  children,
}: {
  pr: PrRef;
  review: ReviewLink;
  readIndex: boolean;
  children: ReactNode;
}) {
  const { analysis_id: id, analysed_head: head } = review;
  const { data } = useFiles(id, readIndex);
  const current = data?.current_head == null || data.current_head === head;
  return (
    <FilePathLinks analysisId={id} pr={pr} files={current ? data?.files : undefined}>
      {children}
    </FilePathLinks>
  );
}
