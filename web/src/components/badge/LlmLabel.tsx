import type { Classification, FlowGroup, FragmentKind, RiskBand } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";

// LLM labels: plain text with a glyph, no badge border; each says in its tooltip that it is LLM output.
const BASE =
  "inline-block cursor-help rounded px-1.5 text-[11px] leading-[18px] font-semibold whitespace-nowrap";

const RISK: Record<RiskBand, { text: string; tip: string; look: string }> = {
  high: {
    text: "▲ HIGH",
    tip: "Risk HIGH at system scale: a production user or operator would clearly notice. LLM rating; the reason is in the assessment.",
    look: "bg-high-bg text-high",
  },
  medium: {
    text: "◆ MEDIUM",
    tip: "Risk MEDIUM at system scale: noticeable in some situations. LLM rating; the reason is in the assessment.",
    look: "bg-med-bg text-med",
  },
  low: {
    text: "● LOW",
    tip: "Risk LOW at system scale: unlikely to be noticed. LLM rating; the reason is in the assessment.",
    look: "bg-low-bg text-low",
  },
};

function Label({ text, tip, look }: { text: string; tip: string; look: string }) {
  return (
    <Tooltip tip={tip}>
      <span data-llm-label tabIndex={0} className={`${BASE} ${look}`}>
        {text}
      </span>
    </Tooltip>
  );
}

/** LLM risk band of a flow or step; `null` when the LLM gave none. */
export function RiskLabel({ risk }: { risk: RiskBand | null }) {
  if (risk === null) {
    return <Label text="not rated" tip="The LLM did not rate the risk here." look="text-muted" />;
  }
  return <Label {...RISK[risk]} />;
}

const words = (value: string) => value.replaceAll("_", " ");

/** LLM classification of a flow. */
export function ClassificationLabel({ value }: { value: Classification }) {
  return (
    <Label
      text={words(value)}
      tip="Classification of the flow by the LLM; not a deterministic check."
      look="font-mono font-medium text-muted"
    />
  );
}

/** LLM kind of a step (modified, added, moved, …). */
export function KindLabel({ kind }: { kind: FragmentKind }) {
  return (
    <Label
      text={words(kind)}
      tip="Kind of step as the LLM describes it; the changed lines themselves are marked in the gutter from the diff."
      look="bg-surface-2 text-text"
    />
  );
}

const GROUP: Partial<Record<FlowGroup, { text: string; tip: string }>> = {
  llm_classified_refactor: {
    text: "LLM-classified refactor",
    tip: "The LLM says this flow only restructures code. No deterministic check confirms it, so it is walked like any flow.",
  },
  llm_classified_tests: {
    text: "LLM-classified tests",
    tip: "The LLM says this flow is test code inside production files. No path rule recognised it, so it is shown, not hidden.",
  },
};

/** Label of an LLM-classified refactor or test flow; nothing for behaviour flows. */
export function GroupLabel({ group }: { group: FlowGroup }) {
  const label = GROUP[group];
  return label ? <Label {...label} look="bg-llm-bg text-llm" /> : null;
}
