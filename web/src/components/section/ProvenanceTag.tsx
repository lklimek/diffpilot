import { Tooltip } from "../tooltip/Tooltip";

/** Where a piece of content comes from. */
export type ProvenanceSource = "llm" | "github" | "api" | "github_agent";

// Says where text comes from, never that it is correct: not a fact badge (brief §1).
const TAG: Record<ProvenanceSource, { text: string; tip: string; look: string }> = {
  llm: {
    text: "LLM",
    tip: "This text was written by the LLM; it was not checked.",
    look: "bg-llm-bg text-llm",
  },
  github: {
    text: "GitHub",
    tip: "Taken from GitHub as written there.",
    look: "border border-border text-muted",
  },
  api: {
    text: "API",
    tip: "Submitted by an agent through the DiffPilot API.",
    look: "border border-border text-muted",
  },
  github_agent: {
    text: "GitHub + agent",
    tip: "GitHub conversation with separately attributed agent annotations.",
    look: "border border-border text-muted",
  },
};

/** What a tag of `source` means, for the tooltip of a control holding tags without tips: "LLM tag: …". */
export const provenanceNote = (source: ProvenanceSource) => `${TAG[source].text} tag: ${TAG[source].tip}`;

/**
 * Small tag naming the source of the content next to it; `focusable: false` leaves it out of the tab order;
 * `tip: false` gives it no tooltip, for a tag inside a control whose own tooltip explains it ({@link provenanceNote}).
 */
export function ProvenanceTag({
  source,
  focusable = true,
  tip: withTip = true,
  detail,
}: {
  source: ProvenanceSource;
  focusable?: boolean;
  tip?: boolean;
  detail?: string;
}) {
  const { text, tip, look } = TAG[source];
  const tag = (
    <span
      data-provenance={source}
      tabIndex={focusable && withTip ? 0 : undefined}
      className={`inline-block rounded px-1 align-baseline text-[10.5px] leading-4 font-semibold whitespace-nowrap ${look} ${withTip ? "cursor-help" : ""}`}
    >
      {text}
    </span>
  );
  return withTip ? <Tooltip tip={detail ? `${tip} ${detail}` : tip}>{tag}</Tooltip> : tag;
}
