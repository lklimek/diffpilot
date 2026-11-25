// TODO(v1.4 phase B): replace with generated contract type (plan C1)
import type { WorkItem } from "../generated/types";

export interface Judgement {
  state: "current" | "outdated" | "queued" | "judging" | "not_judged" | "capped";
  judged_at: string | null;
  outdated_because: "facts_changed" | "prompt_changed" | null;
}

/** Older servers still expose the last judgement and its provisional score. */
export function itemJudgement(item: WorkItem & { judgement?: Judgement }): Judgement {
  return (
    item.judgement ?? {
      state: item.llm
        ? item.llm.inherited || item.score.state === "provisional"
          ? "outdated"
          : "current"
        : "not_judged",
      judged_at: item.llm?.judged_at ?? null,
      outdated_because: null,
    }
  );
}

export const RANKING_DAILY_LIMIT = 120;
