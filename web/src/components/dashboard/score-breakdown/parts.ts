import type { ScoreComponent, ScorePart } from "../../../api/generated/types";

/** Display name of a score component; an unknown one (tolerant reader) shows its wire name. */
const LABEL: Record<ScoreComponent, string> = {
  advancement: "Advancement",
  defect_severity: "Defect severity",
  unblocks: "Unblocks",
  human_waiting: "Human waiting",
  recency: "Recency",
  change_impact: "Change impact",
  completion: "Completion",
  change_size: "Change size",
  urgency_bonus: "Urgency",
  blocked_penalty: "Blocked penalty",
  default_branch_bonus: "Default branch",
  milestone_bonus: "Milestone",
};

/** Name of a part's component. */
export const partLabel = (component: ScoreComponent): string =>
  LABEL[component] ?? String(component).replaceAll("_", " ");

/** How a part's source is shown: `llm` adds the LLM tag; `note` follows it (or stands alone). */
export interface SourceView {
  llm: boolean;
  note: string | null;
  tip: string;
}

/** Source of a part as text: who decided it and how final it is (every part names its source, ADR-016). */
export function sourceView(part: ScorePart, judgedOn: string | null): SourceView {
  if (part.source === "not_applicable") {
    return { llm: false, note: "not applicable", tip: "Does not apply to this kind of item; it adds 0." };
  }
  if (part.status === "estimated") {
    return {
      llm: false,
      note: "estimate",
      tip: "A fixed estimate until the LLM judges this item; the basis names the rule.",
    };
  }
  if (part.status === "pending") {
    return {
      llm: part.source === "llm",
      note: "pending",
      tip: "Not known yet; it adds nothing until it is.",
    };
  }
  if (part.source === "computed") {
    return { llm: false, note: "computed", tip: "Computed by the server from the scan facts." };
  }
  if (part.status === "inherited") {
    const from = judgedOn ? `from ${judgedOn}, ` : "";
    return {
      llm: true,
      note: `${from}provisional`,
      tip: "Judged by the LLM before the item changed; carried over until the LLM judges it again.",
    };
  }
  if (part.status === "overridden") {
    return {
      llm: true,
      note: "replaced the computed default",
      tip: "The LLM replaced the computed default; the basis says why.",
    };
  }
  return { llm: true, note: null, tip: "Judged by the LLM; the basis says why." };
}
