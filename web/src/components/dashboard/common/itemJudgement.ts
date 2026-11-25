import type { Judgement, WorkItem } from "../../../api/generated/types";

/** Older servers still expose the last judgement and its provisional score. */
export function itemJudgement(item: WorkItem): Judgement {
  return (
    item.judgement ?? {
      state: item.llm ? (item.llm.inherited ? "outdated" : "current") : "not_judged",
      judged_at: item.llm?.judged_at ?? null,
      outdated_because: null,
    }
  );
}
